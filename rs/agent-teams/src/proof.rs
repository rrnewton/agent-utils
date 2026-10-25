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
    let text = std::fs::read_to_string(registry.join(agent).join("agent.json")).ok()?;
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
    let Some(coordinator) = coordinator_identity(designation, registry) else {
        return SenderStanding::Unknown;
    };
    if ancestry_contains(&coordinator) {
        return SenderStanding::Coordinator;
    }
    // Not the coordinator: is the caller at least a known agent of the registry?
    if let (Designation::Registry, Some(registry)) = (designation, registry) {
        if let Ok(entries) = std::fs::read_dir(registry) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Some((identity, _)) = agent_record(registry, &name) {
                    if ancestry_contains(&identity) {
                        let lieutenant = std::fs::read_dir(registry)
                            .map(|all| {
                                all.flatten().any(|other| {
                                    agent_record(registry, &other.file_name().to_string_lossy())
                                        .and_then(|(_, parent)| parent)
                                        .is_some_and(|parent| parent == name)
                                })
                            })
                            .unwrap_or(false);
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
    let Ok(entries) = std::fs::read_dir(registry) else {
        return false;
    };
    entries.flatten().any(|entry| {
        agent_record(registry, &entry.file_name().to_string_lossy())
            .is_some_and(|(identity, _)| ancestry_contains(&identity))
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
}
