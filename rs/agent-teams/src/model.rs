//! The read-only model (phase P1): `teams.json`, `routes.d/*.json` and per-host
//! `declaration.json`, parsed as typed views over the raw JSON so `comment` and
//! `notes` survive machine rewrites (design sections 5 and 6).

use crate::jsonfile::{check_schema, read_json, unknown_keys, DataError, DataResult};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A team name must match `^[a-z0-9-]{1,32}$` (design section 5).
pub fn valid_team_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A host key must fit the team-tag grammar (design section 8.1):
/// starts alphanumeric, then alphanumerics, `-` or `.`, at most 63 bytes.
pub fn valid_host_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    match bytes.next() {
        Some(b) if b.is_ascii_lowercase() || b.is_ascii_digit() => {}
        _ => return false,
    }
    name.len() <= 63
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
}

/// One contact of a team entry: a destination with its own dedupe authority.
#[derive(Debug, Clone, PartialEq)]
pub enum Contact {
    /// The entry host's `agent-teams accept`, whatever its deliverer.
    Host,
    /// An agent-runtime session reached through a session-mode command route.
    Session {
        /// Route name that reaches the session.
        route: String,
        /// Runtime session identifier.
        session: String,
    },
}

impl Contact {
    /// Short label for tables and logs.
    pub fn label(&self) -> String {
        match self {
            Contact::Host => "host".to_string(),
            Contact::Session { session, .. } => format!("session:{session}"),
        }
    }
}

/// One entry: the coordinator of one team on one host, with ordered contacts.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Host key into `hosts`.
    pub host: String,
    /// Informational coordinator name; the host's declaration is authoritative.
    pub coordinator: Option<String>,
    /// Ordered contacts.
    pub contacts: Vec<Contact>,
}

/// One team in `teams.json`.
#[derive(Debug, Clone)]
pub struct Team {
    /// Team name.
    pub name: String,
    /// Human description.
    pub description: Option<String>,
    /// Preferred host when `send TEAM` names no host.
    pub primary: Option<String>,
    /// Entries, in file order.
    pub entries: Vec<Entry>,
    /// Last human or `discover --apply` confirmation (raw object).
    pub reviewed: Option<Value>,
}

/// One host in `teams.json`.
#[derive(Debug, Clone)]
pub struct Host {
    /// Host key.
    pub name: String,
    /// Ordered route names to try for `host` contacts on this host.
    pub routes: Vec<String>,
    /// Path of the `agent-teams` binary on that host.
    pub agent_teams: Option<String>,
    /// Registered services on this host: name -> systemd unit.
    pub services: BTreeMap<String, String>,
}

/// The parsed `teams.json`, keeping the raw value for rewrites.
#[derive(Debug, Clone)]
pub struct TeamsFile {
    /// Raw JSON, with comments and key order.
    pub raw: Value,
    /// Hosts by key.
    pub hosts: BTreeMap<String, Host>,
    /// Teams by name.
    pub teams: BTreeMap<String, Team>,
    /// Star topology hubs in order (design section 11.5); empty when unconfigured.
    pub hubs: Vec<String>,
    /// Unknown top-level keys (warnings; errors under `--strict`).
    pub warnings: Vec<String>,
}

const TEAMS_KNOWN: &[&str] = &["schema", "hosts", "teams", "topology", "trials"];
const HOST_KNOWN: &[&str] = &["routes", "agent_teams", "services"];
const TEAM_KNOWN: &[&str] = &[
    "description",
    "primary",
    "entries",
    "policy_hint",
    "reviewed",
];
const ENTRY_KNOWN: &[&str] = &["host", "coordinator", "contacts"];
const CONTACT_KNOWN: &[&str] = &["kind", "route", "session"];

/// Load and validate a `teams.json`.
pub fn load_teams(path: &Path) -> DataResult<TeamsFile> {
    let raw = read_json(path)?;
    check_schema(&raw, "teams", path)?;
    parse_teams(raw, path)
}

/// Parse a teams value (also used by `import` for its proposal).
pub fn parse_teams(raw: Value, path: &Path) -> DataResult<TeamsFile> {
    let obj = raw
        .as_object()
        .ok_or_else(|| DataError::new("parse", format!("{}: not an object", path.display())))?;
    let mut warnings = Vec::new();
    for key in unknown_keys(obj, TEAMS_KNOWN) {
        warnings.push(format!("teams.json: unknown key {key:?}"));
    }
    let mut hosts = BTreeMap::new();
    if let Some(hosts_obj) = obj.get("hosts").and_then(Value::as_object) {
        for (name, value) in hosts_obj {
            if !valid_host_name(name) {
                warnings.push(format!("host {name:?} does not fit the team-tag grammar"));
            }
            let host_obj = value.as_object().cloned().unwrap_or_default();
            for key in unknown_keys(&host_obj, HOST_KNOWN) {
                warnings.push(format!("host {name}: unknown key {key:?}"));
            }
            let routes = string_list(host_obj.get("routes"));
            let mut services = BTreeMap::new();
            if let Some(svc) = host_obj.get("services").and_then(Value::as_object) {
                for (svc_name, svc_value) in svc {
                    if let Some(unit) = svc_value.get("unit").and_then(Value::as_str) {
                        services.insert(svc_name.clone(), unit.to_string());
                    }
                }
            }
            hosts.insert(
                name.clone(),
                Host {
                    name: name.clone(),
                    routes,
                    agent_teams: host_obj
                        .get("agent_teams")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    services,
                },
            );
        }
    }
    let mut teams = BTreeMap::new();
    if let Some(teams_obj) = obj.get("teams").and_then(Value::as_object) {
        for (name, value) in teams_obj {
            if !valid_team_name(name) {
                return Err(DataError::new(
                    "parse",
                    format!(
                        "{}: team name {name:?} must match ^[a-z0-9-]{{1,32}}$",
                        path.display()
                    ),
                ));
            }
            let team_obj = value.as_object().cloned().unwrap_or_default();
            for key in unknown_keys(&team_obj, TEAM_KNOWN) {
                warnings.push(format!("team {name}: unknown key {key:?}"));
            }
            let mut entries = Vec::new();
            if let Some(list) = team_obj.get("entries").and_then(Value::as_array) {
                for item in list {
                    let entry_obj = item.as_object().cloned().unwrap_or_default();
                    for key in unknown_keys(&entry_obj, ENTRY_KNOWN) {
                        warnings.push(format!("team {name} entry: unknown key {key:?}"));
                    }
                    let host = entry_obj
                        .get("host")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let mut contacts = Vec::new();
                    if let Some(list) = entry_obj.get("contacts").and_then(Value::as_array) {
                        for contact in list {
                            let contact_obj = contact.as_object().cloned().unwrap_or_default();
                            for key in unknown_keys(&contact_obj, CONTACT_KNOWN) {
                                warnings.push(format!("team {name} contact: unknown key {key:?}"));
                            }
                            match contact_obj.get("kind").and_then(Value::as_str) {
                                Some("host") => contacts.push(Contact::Host),
                                Some("session") => contacts.push(Contact::Session {
                                    route: contact_obj
                                        .get("route")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                    session: contact_obj
                                        .get("session")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                }),
                                other => warnings.push(format!(
                                    "team {name}: contact kind {other:?} is not host or session"
                                )),
                            }
                        }
                    }
                    entries.push(Entry {
                        host,
                        coordinator: entry_obj
                            .get("coordinator")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        contacts,
                    });
                }
            }
            teams.insert(
                name.clone(),
                Team {
                    name: name.clone(),
                    description: team_obj
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    primary: team_obj
                        .get("primary")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    entries,
                    reviewed: team_obj.get("reviewed").cloned(),
                },
            );
        }
    }
    let hubs = obj
        .get("topology")
        .and_then(|t| t.get("hubs"))
        .map(|v| string_list(Some(v)))
        .unwrap_or_default();
    Ok(TeamsFile {
        raw,
        hosts,
        teams,
        hubs,
        warnings,
    })
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

impl TeamsFile {
    /// Pick the entry for `TEAM[@HOST]`: the named host, else this host, else
    /// `primary`, else refuse (design question 2, recommendation).
    pub fn pick_entry<'a>(
        &'a self,
        team: &str,
        host: Option<&str>,
        this_host: &str,
    ) -> DataResult<&'a Entry> {
        let team_obj = self.teams.get(team).ok_or_else(|| {
            DataError::new("unknown-team", format!("no team {team:?} in teams.json"))
        })?;
        if let Some(host) = host {
            return team_obj
                .entries
                .iter()
                .find(|e| e.host == host)
                .ok_or_else(|| {
                    DataError::new(
                        "unknown-entry",
                        format!("team {team} has no entry on host {host}"),
                    )
                });
        }
        if let Some(entry) = team_obj.entries.iter().find(|e| e.host == this_host) {
            return Ok(entry);
        }
        if let Some(primary) = &team_obj.primary {
            if let Some(entry) = team_obj.entries.iter().find(|e| &e.host == primary) {
                return Ok(entry);
            }
        }
        if team_obj.entries.len() == 1 {
            return Ok(&team_obj.entries[0]);
        }
        Err(DataError::new(
            "ambiguous-entry",
            format!("team {team} has entries on several hosts; name one as {team}@HOST"),
        ))
    }
}

/// One route definition from `routes.d`.
#[derive(Debug, Clone)]
pub struct RouteDef {
    /// Route name.
    pub name: String,
    /// Driver: `local`, `ssh` or `command`.
    pub driver: String,
    /// Raw definition, interpreted by the driver.
    pub raw: Value,
    /// File it came from.
    pub source: PathBuf,
}

/// All routes found on the search path, in one namespace.
#[derive(Debug, Default)]
pub struct RoutesFile {
    /// Routes by name.
    pub routes: BTreeMap<String, RouteDef>,
    /// Warnings collected while loading.
    pub warnings: Vec<String>,
}

/// Directory list for route files: the config `routes.d`, then
/// `AGENT_TEAMS_ROUTES_PATH` entries, then `--routes` directories (section 6.3).
pub fn route_dirs(config_dir: &Path, extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = vec![config_dir.join("routes.d")];
    if let Ok(path_var) = std::env::var("AGENT_TEAMS_ROUTES_PATH") {
        for part in path_var.split(':') {
            if !part.is_empty() {
                dirs.push(PathBuf::from(part));
            }
        }
    }
    dirs.extend(extra.iter().cloned());
    dirs
}

/// Load every `*.json` in the route directories, in name order. Files named
/// `*.example.json` are placeholders and are refused as live routes (section 6.3).
pub fn load_routes(dirs: &[PathBuf]) -> DataResult<RoutesFile> {
    let mut out = RoutesFile::default();
    out.routes.insert(
        "local".to_string(),
        RouteDef {
            name: "local".to_string(),
            driver: "local".to_string(),
            raw: Value::Object(Default::default()),
            source: PathBuf::from("<built-in>"),
        },
    );
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort();
        for file in files {
            let file_name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if file_name.ends_with(".example.json") {
                out.warnings.push(format!(
                    "{}: example route file, not loaded as a live route",
                    file.display()
                ));
                continue;
            }
            if file_name.ends_with(".private.json") {
                check_private_ignored(&file, &mut out.warnings);
            }
            let raw = read_json(&file)?;
            check_schema(&raw, "routes", &file)?;
            let Some(routes) = raw.get("routes").and_then(Value::as_object) else {
                out.warnings
                    .push(format!("{}: no routes object", file.display()));
                continue;
            };
            for (name, value) in routes {
                if out.routes.contains_key(name) {
                    return Err(DataError::new(
                        "duplicate-route",
                        format!(
                            "route {name:?} defined twice (second in {})",
                            file.display()
                        ),
                    ));
                }
                let driver = value
                    .get("driver")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                out.routes.insert(
                    name.clone(),
                    RouteDef {
                        name: name.clone(),
                        driver,
                        raw: value.clone(),
                        source: file.clone(),
                    },
                );
            }
        }
    }
    Ok(out)
}

/// Warn when a `*.private.json` sits in a Git working tree without being ignored.
fn check_private_ignored(file: &Path, warnings: &mut Vec<String>) {
    let Some(dir) = file.parent() else { return };
    let status = std::process::Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "check-ignore", "-q", "--"])
        .arg(file)
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(_) => {
            // Not ignored; only warn when inside a working tree at all.
            let inside = std::process::Command::new("git")
                .args([
                    "-C",
                    &dir.to_string_lossy(),
                    "rev-parse",
                    "--is-inside-work-tree",
                ])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if inside {
                warnings.push(format!(
                    "{}: private route file inside a Git working tree but not git-ignored",
                    file.display()
                ));
            }
        }
        Err(_) => {}
    }
}

/// How the coordinator of a declared team is designated on this host.
#[derive(Debug, Clone, PartialEq)]
pub enum Designation {
    /// The agentctl registry's own coordinator designation.
    Registry,
    /// A pinned harness identity (`init --coordinator-self`).
    Pinned {
        /// Boot ID of the pinned process.
        boot_id: String,
        /// Process ID.
        pid: i64,
        /// `/proc/<pid>/stat` start ticks.
        starttime_ticks: i64,
        /// Executable device (0 when the declaration predates recording it).
        executable_device: u64,
        /// Executable inode (0 when the declaration predates recording it).
        executable_inode: u64,
    },
}

/// One `accepts_from` entry of a declaration.
#[derive(Debug, Clone)]
pub struct AcceptRule {
    /// Sender: a team name, or `service:NAME`.
    pub sender: String,
    /// Message kinds accepted from this sender.
    pub kinds: Vec<String>,
    /// `peer` (default) or `operator`.
    pub authority: String,
}

/// A team's declaration on the receiving host (section 6.4).
#[derive(Debug, Clone)]
pub struct Declaration {
    /// Raw JSON.
    pub raw: Value,
    /// Team name.
    pub team: String,
    /// Deliverer kind: `agentctl-queue`, `hook-inbox` or `command`.
    pub deliverer_kind: String,
    /// Deliverer configuration (raw object).
    pub deliverer: Value,
    /// Coordinator designation.
    pub designation: Designation,
    /// Who may send to this team.
    pub accepts_from: Vec<AcceptRule>,
    /// Teams this team's coordinator may send to (empty means unset).
    pub may_send_to: Vec<String>,
    /// Maximum body bytes.
    pub max_bytes: usize,
    /// Maximum TTL in seconds.
    pub max_ttl_seconds: i64,
    /// Default TTL in seconds.
    pub default_ttl_seconds: i64,
    /// Hubs whose relayed envelopes this host accepts (section 11.6).
    pub accepts_relay_from: Vec<String>,
}

/// Load a declaration file.
pub fn load_declaration(path: &Path) -> DataResult<Declaration> {
    let raw = read_json(path)?;
    check_schema(&raw, "declaration", path)?;
    parse_declaration(raw)
}

/// Parse a declaration value.
pub fn parse_declaration(raw: Value) -> DataResult<Declaration> {
    let team = raw
        .get("team")
        .and_then(Value::as_str)
        .ok_or_else(|| DataError::new("parse", "declaration has no team"))?
        .to_string();
    let deliverer = raw.get("deliverer").cloned().unwrap_or(Value::Null);
    let deliverer_kind = deliverer
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("hook-inbox")
        .to_string();
    let designation = match raw
        .get("coordinator")
        .and_then(|c| c.get("designation"))
        .and_then(Value::as_str)
    {
        Some("pinned") => {
            let identity = raw
                .get("coordinator")
                .and_then(|c| c.get("harness_identity"))
                .cloned()
                .unwrap_or(Value::Null);
            Designation::Pinned {
                boot_id: identity
                    .get("boot_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                pid: identity.get("pid").and_then(Value::as_i64).unwrap_or(0),
                starttime_ticks: identity
                    .get("starttime_ticks")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                executable_device: identity
                    .get("executable_device")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                executable_inode: identity
                    .get("executable_inode")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            }
        }
        _ => Designation::Registry,
    };
    let mut accepts_from = Vec::new();
    if let Some(list) = raw.get("accepts_from").and_then(Value::as_array) {
        for item in list {
            accepts_from.push(AcceptRule {
                sender: item
                    .get("sender")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                kinds: string_list(item.get("kinds")),
                authority: item
                    .get("authority")
                    .and_then(Value::as_str)
                    .unwrap_or("peer")
                    .to_string(),
            });
        }
    }
    let limits = raw.get("limits").cloned().unwrap_or(Value::Null);
    Ok(Declaration {
        raw: raw.clone(),
        team,
        deliverer_kind,
        deliverer,
        designation,
        accepts_from,
        may_send_to: string_list(raw.get("may_send_to")),
        max_bytes: limits
            .get("max_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(8192) as usize,
        max_ttl_seconds: limits
            .get("max_ttl_seconds")
            .and_then(Value::as_i64)
            .unwrap_or(604_800),
        default_ttl_seconds: raw
            .get("default_ttl_seconds")
            .and_then(Value::as_i64)
            .unwrap_or(7200),
        accepts_relay_from: string_list(raw.get("accepts_relay_from")),
    })
}

impl Declaration {
    /// Whether `sender` may send a message of `kind` to this team.
    pub fn accepts(&self, sender: &str, kind: &str) -> bool {
        self.accepts_from.iter().any(|rule| {
            rule.sender == sender && (rule.kinds.is_empty() || rule.kinds.iter().any(|k| k == kind))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_validate() {
        assert!(valid_team_name("tools"));
        assert!(valid_team_name("team-1"));
        assert!(!valid_team_name("Tools"));
        assert!(!valid_team_name(""));
        assert!(valid_host_name("host-a"));
        assert!(valid_host_name("host-a.example.net"));
        assert!(!valid_host_name("-host"));
        assert!(!valid_host_name("Host"));
    }

    #[test]
    fn teams_parse_and_pick() {
        let raw = json!({
            "schema": "agent-teams-teams/v1",
            "comment": "kept",
            "hosts": {"host-a": {"routes": ["local"]}, "host-b": {"routes": ["ssh-b"]}},
            "teams": {"tools": {"primary": "host-b", "entries": [
                {"host": "host-a", "contacts": [{"kind": "host"}]},
                {"host": "host-b", "contacts": [{"kind": "host"}, {"kind": "session", "route": "r", "session": "s1"}]}
            ]}}
        });
        let parsed = parse_teams(raw, Path::new("teams.json")).expect("parse");
        assert_eq!(parsed.raw["comment"], "kept");
        let entry = parsed
            .pick_entry("tools", None, "host-a")
            .expect("pick own host");
        assert_eq!(entry.host, "host-a");
        let entry = parsed
            .pick_entry("tools", None, "host-z")
            .expect("pick primary");
        assert_eq!(entry.host, "host-b");
        assert_eq!(entry.contacts.len(), 2);
    }
}
