//! Sender proof (design section 8.2) and coordinator resolution over the
//! control tree.
//!
//! Control is a tree: the registry's designated coordinator at the root,
//! lieutenants below it (`managed_by` = root) managing workers in turn.
//! Cross-team sending stays with the root coordinator and registered
//! services: a lieutenant that needs another team asks its coordinator.
//! The receiving side likewise delivers to the designated coordinator,
//! never to whoever happened to start the receiver.

use crate::jsonfile::DataResult;
use crate::model::Designation;
use agent_queue::identity::{liveness, observe, Ancestry, ProcessLiveness};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// One process identity, as recorded in agentctl's `harness_identity`:
/// the shared `agent-queue` type, so sender proof here and agentctl's own
/// pinning can never disagree about what names a process generation.
pub use agent_queue::identity::ProcessIdentity;

/// Identity of a live process, observed coherently (pidfd-backed), if it exists.
pub fn identity_of(pid: u64) -> Option<ProcessIdentity> {
    observe(pid).ok().map(|live| live.identity)
}

/// Whether a recorded identity still names the same live process. A full
/// identity uses the crate's liveness probe; a partial one recorded before
/// executable fields were stored falls back to boot + start-ticks equality
/// against a fresh observation, which is the weaker proof it was pinned with.
pub fn identity_alive(identity: &ProcessIdentity) -> bool {
    if identity.valid() {
        return liveness(identity) == ProcessLiveness::Alive;
    }
    identity_of(identity.pid).is_some_and(|live| {
        live.boot_id == identity.boot_id && live.starttime_ticks == identity.starttime_ticks
    })
}

/// The ancestor PIDs of this process, nearest first (including self).
pub fn ancestry() -> Vec<u64> {
    Ancestry::of_self()
        .map(|ancestry| ancestry.links.iter().map(|link| link.pid).collect())
        .unwrap_or_else(|_| vec![u64::from(std::process::id())])
}

/// Whether any ancestor (or self) has exactly this identity: the crate's
/// double-walked ancestry, compared on boot, PID and start ticks.
pub fn ancestry_contains(identity: &ProcessIdentity) -> bool {
    Ancestry::of_self()
        .map(|ancestry| ancestry.contains(identity))
        .unwrap_or(false)
}

/// Parse a recorded harness identity: the full `agent-queue` shape when
/// present, else the older boot/PID/ticks subset (executable fields zero).
fn parse_identity(value: &Value) -> Option<ProcessIdentity> {
    if let Ok(full) = serde_json::from_value::<ProcessIdentity>(value.clone()) {
        return Some(full);
    }
    Some(ProcessIdentity {
        version: 1,
        boot_id: value.get("boot_id")?.as_str()?.to_string(),
        pid: value.get("pid")?.as_u64()?,
        starttime_ticks: value.get("starttime_ticks")?.as_u64()?,
        executable_device: value
            .get("executable_device")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        executable_inode: value
            .get("executable_inode")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

/// The agentctl binary used for designation reads: `$AGENT_TEAMS_AGENTCTL`,
/// else `agentctl` on PATH.
/// First executable named `name` on the `:`-separated `path_var`, if any.
fn find_on_path(name: &str, path_var: &std::ffi::OsStr) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The agentctl binary to shell out to: `$AGENT_TEAMS_AGENTCTL`, else
/// `agentctl` on PATH, else the per-user install locations. The PATH step
/// matters because serve usually runs under a non-login ssh whose PATH
/// lacks `~/bin` and `~/.local/bin` - the first helper-process auto-drain
/// died exactly there (live trial 5b, 2026-10-25).
pub(crate) fn agentctl_binary() -> PathBuf {
    if let Some(chosen) = std::env::var_os("AGENT_TEAMS_AGENTCTL") {
        return PathBuf::from(chosen);
    }
    if let Some(path) = std::env::var_os("PATH").and_then(|paths| find_on_path("agentctl", &paths))
    {
        return path;
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for candidate in [home.join(".local/bin/agentctl"), home.join("bin/agentctl")] {
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from("agentctl")
}

/// PATH for a spawned agentctl: ours, with the per-user bin directories
/// prepended when missing. agentctl in turn shells out by bare name (for
/// example `herdr`), and a non-login ssh PATH lacks both directories -
/// live trial 5c's first drain failed exactly there, after the same fix
/// one level up found agentctl itself.
fn augmented_path(
    current: Option<&std::ffi::OsStr>,
    home: Option<&Path>,
) -> Option<std::ffi::OsString> {
    let home = home?;
    let mut dirs = vec![home.join(".local/bin"), home.join("bin")];
    if let Some(current) = current {
        dirs.extend(std::env::split_paths(current));
    }
    std::env::join_paths(dirs).ok()
}

/// A `Command` for agentctl with [`augmented_path`] applied.
pub(crate) fn agentctl_command() -> std::process::Command {
    let mut command = std::process::Command::new(agentctl_binary());
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(path) = augmented_path(std::env::var_os("PATH").as_deref(), home.as_deref()) {
        command.env("PATH", path);
    }
    command
}

/// Whether an `agentctl capabilities` document lists the `inbox.v2` queue
/// lane (phase P2). Team items land in that lane, so an agentctl without
/// it queues them where no drainer ever types them.
fn lanes_supported(capabilities: &Value) -> bool {
    capabilities
        .get("queue")
        .and_then(|queue| queue.get("lanes"))
        .and_then(Value::as_array)
        .is_some_and(|lanes| lanes.iter().any(|lane| lane.as_str() == Some("inbox.v2")))
}

/// Ask the configured agentctl whether it supports queue lanes: `Some`
/// when it answered, `None` when it could not be run or parsed.
pub(crate) fn agentctl_supports_lanes() -> Option<bool> {
    let output = agentctl_command().arg("capabilities").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let document: Value = serde_json::from_slice(&output.stdout).ok()?;
    Some(lanes_supported(&document))
}

/// `agentctl coordinator show` as JSON: the designation as agentctl itself
/// resolves it (following the designated generation through rename/revive),
/// rather than the raw file. `None` when the binary is unavailable, predates
/// the command, or no coordinator is designated (show exits 1 then).
pub fn coordinator_show(registry: &Path) -> Option<Value> {
    let output = agentctl_command()
        .arg("--registry")
        .arg(registry)
        .args(["coordinator", "show"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    let value: Value = serde_json::from_str(&text[start..=end]).ok()?;
    if value.get("designated").and_then(Value::as_bool) == Some(true) {
        Some(value)
    } else {
        None
    }
}

/// Read an agentctl registry's designated coordinator name: from
/// `agentctl coordinator show` first, falling back to the designation file
/// (`REGISTRY/coordinator.json`, `{"name": ...}`) for older binaries.
pub fn registry_coordinator_name(registry: &Path) -> Option<String> {
    if let Some(show) = coordinator_show(registry) {
        if let Some(name) = show.get("name").and_then(Value::as_str) {
            return Some(name.to_string());
        }
    }
    let value: Value =
        serde_json::from_str(&std::fs::read_to_string(registry.join("coordinator.json")).ok()?)
            .ok()?;
    value.get("name")?.as_str().map(str::to_string)
}

/// Read one agent record's pinned harness identity, plus its `managed_by`
/// controller name, when recorded.
pub fn agent_record(registry: &Path, agent: &str) -> Option<(ProcessIdentity, Option<String>)> {
    // Either registry layout (#593 registry-agents-subdir).
    let directory = agent_queue::registry::directory(registry, agent).ok()?;
    let text = std::fs::read_to_string(directory.join("agent.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    // Records name the acting process either way: Muse panes pin
    // harness_identity, other harnesses custom_process_identity (the same
    // fallback agentctl's acting_process uses).
    let identity = value
        .get("harness_identity")
        .filter(|v| !v.is_null())
        .or_else(|| value.get("custom_process_identity"))?;
    let parsed = parse_identity(identity)?;
    let managed_by = value
        .get("managed_by")
        .and_then(|m| m.get("agent"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Some((parsed, managed_by))
}

/// The designated coordinator's identity for a registry designation.
pub fn coordinator_identity(
    designation: &Designation,
    registry: Option<&Path>,
) -> Option<ProcessIdentity> {
    match designation {
        Designation::Pinned {
            boot_id,
            pid,
            starttime_ticks,
            executable_device,
            executable_inode,
        } => Some(ProcessIdentity {
            version: 1,
            boot_id: boot_id.clone(),
            pid: u64::try_from(*pid).ok()?,
            starttime_ticks: u64::try_from(*starttime_ticks).ok()?,
            executable_device: *executable_device,
            executable_inode: *executable_inode,
        }),
        Designation::Registry => {
            let registry = registry?;
            let name = registry_coordinator_name(registry)?;
            if let Some((identity, _)) = agent_record(registry, &name) {
                return Some(identity);
            }
            // The record carries no pinned identity (for example a session
            // registered without one): observe the harness process agentctl
            // itself reports for the designated coordinator, this boot only.
            let pid = coordinator_show(registry)?
                .get("harness_pid")
                .and_then(Value::as_u64)?;
            identity_of(pid)
        }
    }
}

/// Agentcloud facts recorded for one agent: its session and the
/// orchestrator endpoint every probe of that session must use.
#[derive(Debug, Clone, PartialEq)]
pub struct CloudInfo {
    /// The agentcloud session ID (`session_value` in the record).
    pub session_id: String,
    /// Recorded orchestrator endpoint (`agentcloud.endpoint`), when present.
    pub endpoint: Option<String>,
}

/// The agentcloud facts recorded for `agent`, when it is an agentcloud
/// session. Unlike [`agent_record`], this needs no pinned process identity:
/// an agentcloud record carries `harness_identity: null` and
/// `harness_pid: null` by design, because the orchestrator spawns a fresh
/// harness process per turn, so no local pid can name the session.
pub fn cloud_info(registry: &Path, agent: &str) -> Option<CloudInfo> {
    let directory = agent_queue::registry::directory(registry, agent).ok()?;
    let text = std::fs::read_to_string(directory.join("agent.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let cloud = value.get("agentcloud").cloned().unwrap_or(Value::Null);
    if value.get("harness").and_then(Value::as_str) != Some("agentcloud")
        && value.get("adapter").and_then(Value::as_str) != Some("agentcloud")
        && cloud.is_null()
    {
        return None;
    }
    let session_id = value
        .get("session_value")
        .and_then(Value::as_str)
        .filter(|session| !session.is_empty())?
        .to_string();
    let endpoint = cloud
        .get("endpoint")
        .and_then(Value::as_str)
        .filter(|endpoint| !endpoint.is_empty())
        .map(str::to_string);
    Some(CloudInfo {
        session_id,
        endpoint,
    })
}

/// Whether an agentcloud session can receive work, as its orchestrator
/// reports it. `Unreachable` is deliberately distinct from `NotLive`: the
/// probe itself failed (binary missing, timeout, non-zero exit, unparsable
/// output), so liveness is unverified rather than disproved, and callers
/// must say so instead of silently passing or failing the coordinator.
#[derive(Debug, Clone, PartialEq)]
pub enum CloudLiveness {
    /// The session is active: running, attached, or waiting for input.
    Live,
    /// The orchestrator answered and the session is absent or inactive.
    NotLive,
    /// The orchestrator could not be asked; liveness is unverified.
    Unreachable(String),
}

/// Judge one session from an `agentcloudctl list` document (a JSON array
/// of fleet rows). A session counts as live when the orchestrator reports
/// it active: a run in flight, a node attached (its `agentterm` viewer),
/// or an activity other than `idle` (for example `waiting` on a queued
/// first prompt). A merely on-disk row (`running: false`, `activity:
/// "idle"`, nothing attached) is a finished session, not a coordinator.
pub fn parse_cloud_liveness(listing: &Value, session_id: &str) -> CloudLiveness {
    let Some(rows) = listing.as_array() else {
        return CloudLiveness::Unreachable("agentcloudctl list printed no JSON array".to_string());
    };
    let Some(row) = rows
        .iter()
        .find(|row| row.get("session_id").and_then(Value::as_str) == Some(session_id))
    else {
        return CloudLiveness::NotLive;
    };
    let running = row.get("running").and_then(Value::as_bool) == Some(true);
    let attached = row
        .get("attached_nodes")
        .and_then(Value::as_array)
        .is_some_and(|nodes| !nodes.is_empty());
    let active = row
        .get("activity")
        .and_then(Value::as_str)
        .is_some_and(|activity| !activity.is_empty() && activity != "idle");
    if running || attached || active {
        CloudLiveness::Live
    } else {
        CloudLiveness::NotLive
    }
}

/// The outcome of injecting one input into an agentcloud session.
#[derive(Debug, Clone, PartialEq)]
pub enum CloudInject {
    /// The orchestrator journaled the input.
    Injected,
    /// The orchestrator refused it before journaling; a retry is safe.
    Refused(String),
    /// The input may have been journaled; never retry it blind.
    Uncertain(String),
}

/// Inject one input into an agentcloud session:
/// `agentcloudctl send --session S --text TEXT --idempotency-key ID
/// --end-of-turn` on the session's recorded endpoint, bounded like a
/// liveness probe. The idempotency key makes a retry after `Refused`
/// journal nothing twice.
pub fn inject_cloud_input(info: &CloudInfo, id: &str, text: &str) -> CloudInject {
    let mut command = std::process::Command::new(agentcloudctl_binary());
    command.arg("send");
    if let Some(endpoint) = &info.endpoint {
        command.args(["--ws-url", endpoint]);
    }
    command.args([
        "--session",
        &info.session_id,
        "--text",
        text,
        "--idempotency-key",
        id,
        "--end-of-turn",
    ]);
    match run_bounded(command, CLOUD_PROBE_TIMEOUT * 2) {
        Ok((true, _)) => CloudInject::Injected,
        Ok((false, _)) => CloudInject::Refused(
            "agentcloudctl send exited non-zero; the session refused or could not take the input"
                .to_string(),
        ),
        Err(reason) => CloudInject::Uncertain(reason),
    }
}

/// The `agentcloudctl` binary used for session probes:
/// `$AGENT_TEAMS_AGENTCLOUDCTL`, else `agentcloudctl` on PATH.
fn agentcloudctl_binary() -> PathBuf {
    if let Some(chosen) = std::env::var_os("AGENT_TEAMS_AGENTCLOUDCTL") {
        return PathBuf::from(chosen);
    }
    PathBuf::from("agentcloudctl")
}

/// How long one agentcloud liveness probe may run. The orchestrator is a
/// network service; an unbounded probe would hang `check` exactly when it
/// is down, which is when the answer matters most.
const CLOUD_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Run `command` to completion under `timeout`, returning its success and
/// stdout. The child's stdout is drained on its own thread so a verbose
/// listing can never fill the pipe and stall the probe past its bound.
fn run_bounded(
    mut command: std::process::Command,
    timeout: std::time::Duration,
) -> Result<(bool, String), String> {
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot run probe: {error}"))?;
    let reader = child.stdout.take().map(|mut stdout| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut stdout, &mut text);
            text
        })
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("probe timed out after {}s", timeout.as_secs()));
            }
            Err(error) => return Err(format!("probe wait failed: {error}")),
        }
    };
    let stdout = reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    Ok((status.success(), stdout))
}

/// Probe one agentcloud session's liveness through `agentcloudctl list`,
/// bounded by [`CLOUD_PROBE_TIMEOUT`]. The recorded endpoint is passed
/// explicitly so the probe reaches the orchestrator the session lives on.
pub fn probe_cloud_liveness(info: &CloudInfo) -> CloudLiveness {
    probe_cloud_liveness_with(&agentcloudctl_binary(), info, CLOUD_PROBE_TIMEOUT)
}

fn probe_cloud_liveness_with(
    ctl: &Path,
    info: &CloudInfo,
    timeout: std::time::Duration,
) -> CloudLiveness {
    let mut command = std::process::Command::new(ctl);
    command.arg("list");
    if let Some(endpoint) = &info.endpoint {
        command.args(["--ws-url", endpoint]);
    }
    match run_bounded(command, timeout) {
        Ok((true, stdout)) => match serde_json::from_str::<Value>(&stdout) {
            Ok(listing) => parse_cloud_liveness(&listing, &info.session_id),
            Err(error) => CloudLiveness::Unreachable(format!(
                "agentcloudctl list printed invalid JSON: {error}"
            )),
        },
        Ok((false, _)) => CloudLiveness::Unreachable(
            "agentcloudctl list exited non-zero; orchestrator unreachable or refused the probe"
                .to_string(),
        ),
        Err(reason) => CloudLiveness::Unreachable(reason),
    }
}

/// Liveness of one registered agent's agentcloud session, when it is one.
pub fn cloud_liveness(registry: &Path, agent: &str) -> Option<CloudLiveness> {
    let info = cloud_info(registry, agent)?;
    Some(probe_cloud_liveness(&info))
}

/// Whether a designation's coordinator can receive work, however it is
/// hosted: a pinned or Herdr-hosted coordinator by its process identity,
/// an agentcloud coordinator by its orchestrator-reported session state.
#[derive(Debug, Clone, PartialEq)]
pub enum CoordinatorLiveness {
    /// The coordinator can receive work now.
    Live,
    /// The coordinator is verifiably gone or inactive.
    NotLive,
    /// Liveness could not be verified (the orchestrator is unreachable);
    /// distinct from both answers above, never a silent pass.
    Unreachable(String),
}

/// Liveness of the coordinator a designation names.
pub fn coordinator_liveness(
    designation: &Designation,
    registry: Option<&Path>,
) -> CoordinatorLiveness {
    if let Designation::Registry = designation {
        if let Some(registry) = registry {
            if let Some(name) = registry_coordinator_name(registry) {
                if let Some(liveness) = cloud_liveness(registry, &name) {
                    return match liveness {
                        CloudLiveness::Live => CoordinatorLiveness::Live,
                        CloudLiveness::NotLive => CoordinatorLiveness::NotLive,
                        CloudLiveness::Unreachable(reason) => {
                            CoordinatorLiveness::Unreachable(reason)
                        }
                    };
                }
            }
        }
    }
    match coordinator_identity(designation, registry) {
        Some(identity) if identity_alive(&identity) => CoordinatorLiveness::Live,
        _ => CoordinatorLiveness::NotLive,
    }
}

/// Walk `managed_by` from `agent` up to the tree root; returns the chain
/// (agent first, root last). Cycles stop the walk.
pub fn managed_by_chain(registry: &Path, agent: &str) -> Vec<String> {
    let mut chain = vec![agent.to_string()];
    let mut current = agent.to_string();
    for _ in 0..64 {
        match agent_record(registry, &current).and_then(|(_, parent)| parent) {
            Some(parent) if !chain.contains(&parent) => {
                chain.push(parent.clone());
                current = parent;
            }
            _ => break,
        }
    }
    chain
}

/// How the caller stands to a team's coordinator.
#[derive(Debug, PartialEq)]
pub enum SenderStanding {
    /// The caller is (inside) the designated root coordinator.
    Coordinator,
    /// The caller is an agent of the team but not the root coordinator.
    NotCoordinator {
        /// The agent the caller proved to be.
        agent: String,
        /// Whether that agent manages others (a lieutenant).
        lieutenant: bool,
    },
    /// No agent of this team was found in the ancestry.
    Unknown,
}

/// Prove the caller's standing for one declaration on this host.
pub fn sender_standing(designation: &Designation, registry: Option<&Path>) -> SenderStanding {
    // An agentcloud coordinator has no local harness process to find in
    // the ancestry: the caller proves it is the coordinator by running
    // inside the designated session itself (`$AGENTCLOUD_SESSION_ID`,
    // stamped into every execution the session drives).
    if let (Designation::Registry, Some(registry)) = (designation, registry) {
        if let Some(name) = registry_coordinator_name(registry) {
            if let Some(info) = cloud_info(registry, &name) {
                let caller = std::env::var("AGENTCLOUD_SESSION_ID").unwrap_or_default();
                if !caller.is_empty() && caller == info.session_id {
                    return SenderStanding::Coordinator;
                }
            }
        }
    }
    let Some(coordinator) = coordinator_identity(designation, registry) else {
        return SenderStanding::Unknown;
    };
    if ancestry_contains(&coordinator) {
        return SenderStanding::Coordinator;
    }
    // Not the coordinator: is the caller at least a known agent of the registry?
    if let (Designation::Registry, Some(registry)) = (designation, registry) {
        if let Ok(names) = agent_queue::registry::names(registry) {
            for name in names.iter().cloned() {
                if let Some((identity, _)) = agent_record(registry, &name) {
                    if ancestry_contains(&identity) {
                        let lieutenant = names.iter().any(|other| {
                            agent_record(registry, other)
                                .and_then(|(_, parent)| parent)
                                .is_some_and(|parent| parent == name)
                        });
                        return SenderStanding::NotCoordinator {
                            agent: name,
                            lieutenant,
                        };
                    }
                }
            }
        }
    }
    SenderStanding::Unknown
}

/// Whether this process runs inside the systemd unit `unit` (service proof).
pub fn inside_unit(unit: &str) -> bool {
    std::fs::read_to_string("/proc/self/cgroup")
        .map(|text| {
            text.lines()
                .any(|line| line.split('/').any(|component| component == unit))
        })
        .unwrap_or(false)
}

/// Whether the ancestry contains any known agent harness process of a registry
/// (a worker refused as itself cannot retry as a service).
pub fn ancestry_has_agent(registry: &Path) -> bool {
    let Ok(names) = agent_queue::registry::names(registry) else {
        return false;
    };
    names.iter().any(|name| {
        agent_record(registry, name).is_some_and(|(identity, _)| ancestry_contains(&identity))
    })
}

/// The state directory: `$AGENT_TEAMS_STATE_DIR`, else
/// `$XDG_STATE_HOME/agent-teams`, else `~/.local/state/agent-teams`.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AGENT_TEAMS_STATE_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(xdg).join("agent-teams");
    }
    crate::jsonfile::expand_home("~/.local/state/agent-teams")
}

/// The config directory holding `teams.json` and `routes.d`.
pub fn config_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("agent-teams");
    }
    crate::jsonfile::expand_home("~/.config/agent-teams")
}

/// This host's key: `$AGENT_TEAMS_HOST`, else the machine hostname's first label.
pub fn this_host() -> String {
    if let Ok(host) = std::env::var("AGENT_TEAMS_HOST") {
        if !host.is_empty() {
            return host;
        }
    }
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| {
            s.trim()
                .split('.')
                .next()
                .unwrap_or("localhost")
                .to_string()
        })
        .unwrap_or_else(|_| "localhost".to_string())
}

/// Load every declaration under `<state>/teams.d/*/declaration.json`.
pub fn load_declarations(
    state: &Path,
) -> DataResult<Vec<(String, PathBuf, crate::model::Declaration)>> {
    let mut out = Vec::new();
    let dir = state.join("teams.d");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(out);
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path().join("declaration.json"))
        .filter(|p| p.exists())
        .collect();
    paths.sort();
    for path in paths {
        let declaration = crate::model::load_declaration(&path)?;
        out.push((declaration.team.clone(), path, declaration));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_identity_is_alive_and_in_ancestry() {
        let me = identity_of(u64::from(std::process::id())).expect("self identity");
        assert!(me.valid());
        assert!(identity_alive(&me));
        assert!(ancestry_contains(&me));
        let stale = ProcessIdentity {
            starttime_ticks: me.starttime_ticks + 1,
            ..me.clone()
        };
        assert!(!identity_alive(&stale));
        assert!(!ancestry_contains(&stale));
    }

    #[test]
    fn coordinator_show_parses_fake_agentctl() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("agent-teams-fake-agentctl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let script = dir.join("agentctl");
        let body = "#!/bin/sh\necho '{\"designated\": true, \"name\": \"coordinator\", \"harness_pid\": PID}'\n"
            .replace("PID", &std::process::id().to_string());
        {
            let mut file = std::fs::File::create(&script).expect("script");
            file.write_all(body.as_bytes()).expect("script body");
        }
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("script mode");
        // SAFETY: single-threaded section by construction of this test's own
        // env use; other tests here never read AGENT_TEAMS_AGENTCTL.
        unsafe { std::env::set_var("AGENT_TEAMS_AGENTCTL", &script) };
        let show = coordinator_show(Path::new("/nonexistent-registry"));
        unsafe { std::env::remove_var("AGENT_TEAMS_AGENTCTL") };
        let show = show.expect("show JSON");
        assert_eq!(show["name"], "coordinator");
        assert_eq!(show["harness_pid"], u64::from(std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn post_exec_record_identity_is_live_and_proves_the_sender() {
        // The identity a headless record must end with after an exec'ing
        // launcher (#1221): observed stably from the live harness process.
        // This process stands in for that harness.
        let me = agent_queue::identity::observe_stable(
            u64::from(std::process::id()),
            std::time::Duration::from_secs(2),
        )
        .expect("stable identity")
        .identity;
        let dir = std::env::temp_dir().join(format!(
            "agent-teams-post-exec-{}-{}",
            std::process::id(),
            me.starttime_ticks
        ));
        let agent_dir = dir.join("agents").join("coordinator");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        let record = serde_json::json!({
            "name": "coordinator", "harness_identity": me,
            "managed_by": null,
        });
        std::fs::write(
            agent_dir.join("agent.json"),
            serde_json::to_string(&record).expect("record json"),
        )
        .expect("record file");
        std::fs::write(dir.join("coordinator.json"), "{\"name\": \"coordinator\"}")
            .expect("designation file");
        let (identity, _) = agent_record(&dir, "coordinator").expect("record");
        assert_eq!(identity, me);
        // Liveness and sender proof both accept the post-exec identity.
        assert!(identity_alive(&identity));
        assert!(ancestry_contains(&identity));
        assert_eq!(
            coordinator_liveness(&Designation::Registry, Some(&dir)),
            CoordinatorLiveness::Live
        );
        assert_eq!(
            sender_standing(&Designation::Registry, Some(&dir)),
            SenderStanding::Coordinator
        );
        // The pre-exec / stale image of the same PID would not pass: the
        // executable no longer matches, so liveness is not Alive.
        let mut stale = me.clone();
        stale.executable_inode += 1;
        assert!(!identity_alive(&stale));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_identity_parses_and_matches_ancestry() {
        let me = identity_of(u64::from(std::process::id())).expect("self identity");
        let partial = parse_identity(&serde_json::json!({
            "boot_id": me.boot_id, "pid": me.pid, "starttime_ticks": me.starttime_ticks,
        }))
        .expect("partial identity");
        assert!(!partial.valid());
        assert!(identity_alive(&partial));
        assert!(ancestry_contains(&partial));
    }
}

#[cfg(test)]
mod record_tests {
    use super::*;

    #[test]
    fn record_identity_falls_back_to_custom_process() {
        let dir =
            std::env::temp_dir().join(format!("agent-teams-record-test-{}", std::process::id()));
        let agent_dir = dir.join("coordinator");
        std::fs::create_dir_all(&agent_dir).expect("registry");
        let me = identity_of(u64::from(std::process::id())).expect("self identity");
        let record = serde_json::json!({
            "harness_identity": null,
            "custom_process_identity": me,
            "managed_by": {"agent": "root-1"},
        });
        std::fs::write(
            agent_dir.join("agent.json"),
            serde_json::to_string(&record).expect("record json"),
        )
        .expect("record file");
        std::fs::write(dir.join("coordinator.json"), "{\"name\": \"coordinator\"}")
            .expect("designation file");
        let (identity, managed_by) = agent_record(&dir, "coordinator").expect("record");
        assert_eq!(identity.pid, me.pid);
        assert_eq!(managed_by.as_deref(), Some("root-1"));
        let found =
            coordinator_identity(&Designation::Registry, Some(&dir)).expect("coordinator identity");
        assert_eq!(found.pid, me.pid);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_on_path_finds_executables() {
        let dir = std::env::temp_dir().join(format!("at-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("agentctl");
        std::fs::write(&file, "#!/bin/sh\n").expect("file");
        let joined =
            std::env::join_paths([PathBuf::from("/nonexistent"), dir.clone()]).expect("join");
        assert_eq!(find_on_path("agentctl", &joined), Some(file));
        assert_eq!(find_on_path("nope", &joined), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn augmented_path_prepends_user_bins() {
        let home = PathBuf::from("/home/someone");
        let current = std::ffi::OsString::from("/usr/bin:/bin");
        let joined = augmented_path(Some(&current), Some(&home)).expect("path");
        let dirs: Vec<_> = std::env::split_paths(&joined).collect();
        assert_eq!(dirs[0], PathBuf::from("/home/someone/.local/bin"));
        assert_eq!(dirs[1], PathBuf::from("/home/someone/bin"));
        assert_eq!(dirs[2], PathBuf::from("/usr/bin"));
        assert!(augmented_path(None, None).is_none());
    }

    const CLOUD_SESSION: &str = "2bed1f68-4b34-4880-b933-4913caecf0e6";

    fn cloud_registry(session: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agent-teams-cloud-test-{}-{}",
            std::process::id(),
            session.len()
        ));
        let agent_dir = dir.join("agents").join("coord7");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        let record = serde_json::json!({
            "name": "coord7", "harness": "agentcloud", "adapter": "agentcloud",
            "harness_identity": null, "session_value": session,
            "agentcloud": {"endpoint": "wss://orchestrator.example/ws"},
            "managed_by": {"agent": "coord6"},
        });
        std::fs::write(
            agent_dir.join("agent.json"),
            serde_json::to_string(&record).expect("record json"),
        )
        .expect("record file");
        std::fs::write(dir.join("coordinator.json"), "{\"name\": \"coord7\"}")
            .expect("designation file");
        dir
    }

    #[test]
    fn cloud_info_reads_session_without_a_process_identity() {
        let dir = cloud_registry(CLOUD_SESSION);
        let info = cloud_info(&dir, "coord7").expect("cloud info");
        assert_eq!(info.session_id, CLOUD_SESSION);
        assert_eq!(
            info.endpoint.as_deref(),
            Some("wss://orchestrator.example/ws")
        );
        // The same record yields no pinned process identity: that absence
        // is exactly why liveness must come from the orchestrator.
        assert!(agent_record(&dir, "coord7").is_none());
        assert!(coordinator_identity(&Designation::Registry, Some(&dir)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cloud_liveness_judges_orchestrator_rows() {
        let live_running = serde_json::json!([
            {"session_id": CLOUD_SESSION, "running": true, "activity": "working", "attached_nodes": []}
        ]);
        assert_eq!(
            parse_cloud_liveness(&live_running, CLOUD_SESSION),
            CloudLiveness::Live
        );
        let live_attached = serde_json::json!([
            {"session_id": CLOUD_SESSION, "running": false, "activity": "idle", "attached_nodes": ["node-a"]}
        ]);
        assert_eq!(
            parse_cloud_liveness(&live_attached, CLOUD_SESSION),
            CloudLiveness::Live
        );
        let live_waiting = serde_json::json!([
            {"session_id": CLOUD_SESSION, "running": false, "activity": "waiting", "attached_nodes": []}
        ]);
        assert_eq!(
            parse_cloud_liveness(&live_waiting, CLOUD_SESSION),
            CloudLiveness::Live
        );
        let on_disk = serde_json::json!([
            {"session_id": CLOUD_SESSION, "running": false, "activity": "idle", "attached_nodes": []}
        ]);
        assert_eq!(
            parse_cloud_liveness(&on_disk, CLOUD_SESSION),
            CloudLiveness::NotLive
        );
        let absent = serde_json::json!([{"session_id": "someone-else", "running": true}]);
        assert_eq!(
            parse_cloud_liveness(&absent, CLOUD_SESSION),
            CloudLiveness::NotLive
        );
        assert!(matches!(
            parse_cloud_liveness(&serde_json::json!({"rows": []}), CLOUD_SESSION),
            CloudLiveness::Unreachable(_)
        ));
    }

    #[test]
    fn cloud_probe_distinguishes_down_orchestrator_from_dead_session() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("agent-teams-cloud-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let info = CloudInfo {
            session_id: CLOUD_SESSION.to_string(),
            endpoint: None,
        };
        let write_ctl = |name: &str, body: &str| {
            let script = dir.join(name);
            let mut file = std::fs::File::create(&script).expect("script");
            file.write_all(body.as_bytes()).expect("script body");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("script mode");
            script
        };
        let live = write_ctl(
            "ctl-live",
            &format!(
                "#!/bin/sh\necho '[{{\"session_id\": \"{CLOUD_SESSION}\", \"running\": false, \"activity\": \"idle\", \"attached_nodes\": [\"node-a\"]}}]'\n"
            ),
        );
        assert_eq!(
            probe_cloud_liveness_with(&live, &info, std::time::Duration::from_secs(10)),
            CloudLiveness::Live
        );
        let down = write_ctl("ctl-down", "#!/bin/sh\nexit 1\n");
        assert!(matches!(
            probe_cloud_liveness_with(&down, &info, std::time::Duration::from_secs(10)),
            CloudLiveness::Unreachable(_)
        ));
        let garbage = write_ctl("ctl-garbage", "#!/bin/sh\necho 'not json'\n");
        assert!(matches!(
            probe_cloud_liveness_with(&garbage, &info, std::time::Duration::from_secs(10)),
            CloudLiveness::Unreachable(_)
        ));
        let slow = write_ctl("ctl-slow", "#!/bin/sh\nsleep 5\n");
        assert!(matches!(
            probe_cloud_liveness_with(&slow, &info, std::time::Duration::from_millis(200)),
            CloudLiveness::Unreachable(_)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lanes_supported_reads_the_queue_lanes() {
        let with = serde_json::json!({"queue": {"lanes": ["inbox.v2", "cancel"]}});
        let without = serde_json::json!({"queue": {"lanes": []}});
        let older = serde_json::json!({"interactive": {}});
        assert!(lanes_supported(&with));
        assert!(!lanes_supported(&without));
        assert!(!lanes_supported(&older));
    }
}
