//! Command-line entry point: argument parsing, help, and dispatch.

use crate::deliver::deliverer_for;
use crate::envelope::{build_envelope, derived_key, message_id, team_tag};
use crate::jsonfile::{read_json, write_json_atomic, DataError};
use crate::ledger::{accept, fence, list_entries, read_entry, receipt, retract, AcceptContext};
use crate::model::{
    load_declaration, load_routes, load_teams, route_dirs, Declaration, RoutesFile, TeamsFile,
};
use crate::proof::{
    ancestry_has_agent, coordinator_identity, identity_alive, identity_of, inside_unit,
    load_declarations, sender_standing, SenderStanding,
};
use crate::route::{banner, read_frame, route_for, AttemptOutcome, Route};
use crate::sender::{
    exit_code_for, find_by_key, is_terminal, log_event, next_seq, read_outbox, write_outbox,
};
use crate::serve::{handle_verb, probe};
use crate::timeutil::{
    format_rfc3339_ms, now_unix, now_unix_ms, parse_duration_seconds, parse_rfc3339_ms,
};
use serde_json::{json, Value};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

const QUICKSTART: &str = include_str!("embedded_quickstart.md");
const USERGUIDE: &str = include_str!("embedded_userguide.md");

const HELP: &str = "\
agent-teams: message another team's coordinator, across hosts, exactly once

A team is a coordinator plus the workers it manages, on one host. Only a team's
coordinator (proved by process ancestry) or a registered service may send, and
only to another team's coordinator. Every route ends at the receiving host's
own `agent-teams accept`, which claims the message ID in that team's ledger
before handing it to a local deliverer, so retries and second routes cannot
deliver twice.

USAGE
  agent-teams [--teams FILE] [--routes DIR]... <command> ...

READ-ONLY MODEL
  show [TEAM] [--json]                 Teams, hosts, routes and freshness.
  check [TEAM[@HOST]]... [--strict] [--json]
                                       Verify declarations, contacts and routes.
  probe [--json]                       What this host's own tools say (for peers).
  import --coordinators-file FILE --team NAME [--json]
                                       Convert a per-host coordinators file; prints only.

MESSAGING
  init TEAM (--registry R | --coordinator-self [--coordinator-pid PID])
      [--deliver agentctl-queue|hook-inbox|command] [--argv CMD...]
      [--accepts-from SENDER]... [--accepts-relay-from HUB]...
  send TEAM[@HOST] (--file FILE | --text TEXT) [--key KEY | --again]
      [--topic SLUG] [--supersedes KEY]... [--ttl DUR | --expires TIME]
      [--kind info|request|hold|resume] [--private] [--from-service NAME]
      [--from-team TEAM] [--json]
  reply ID (--file FILE | --text TEXT) [--key KEY] [--json]
  retract ID [--json]
  status ID [--json]                   One message, from the outbox and receipts.
  outbox [--json]                      List the outbox.
  pump [--retry-refused] [--json]      Retry deferred and ambiguous messages.
  seen ID                              Has this host's ledger delivered ID?
  resolve ID (--fence | --mark-delivered)   Human only, on the receiving host.
  hook [--team TEAM]                   Harness hook for the hook-inbox deliverer.

RECEIVING HOST (run by routes, not by hand)
  serve --stdio --peer HOST            Forced-command protocol endpoint.
  accept --protocol 1 [--team T] (--envelope - | --envelope-base64 B)
  receipt ID [--team T]                Pure read of the ledger.
  fence ID --team T --not-after TIME   Bar an ID before it arrives.
  drain-one --queue-dir D --registry R --agent A --id ID --ready-timeout S
                                       Bounded delivery-on-arrival helper.

SSH AND LINKS
  ssh enroll --peer HOST [--from ADDR] Make a key; print the authorized_keys line.
  link run ROUTE | up/down ROUTE | status [ROUTE]
                                       Managed persistent ssh links.

DOCS
  quickstart | userguide | help [CMD] | --help | --version

GLOBAL OPTIONS
  --teams FILE     teams.json (default $AGENT_TEAMS_FILE, else
                   $XDG_CONFIG_HOME/agent-teams/teams.json).
  --routes DIR     Extra routes directory, repeatable (after routes.d and
                   $AGENT_TEAMS_ROUTES_PATH).
  --state-dir DIR  State directory (default $AGENT_TEAMS_STATE_DIR, else
                   $XDG_STATE_HOME/agent-teams).
  --json           Machine-readable output where offered.

EXIT STATUS (send, pump, status)
  0 delivered · 75 not settled (accepted, deferred, in-custody) ·
  76 uncertain (ambiguous, possibly_submitted, lost) ·
  65 terminal and undelivered (expired, superseded, retracted, refused) ·
  2 usage or configuration.

EXAMPLES
  agent-teams init tools --registry ~/work/dev-tools/.agentctl --accepts-from sim
  agent-teams send sim --text 'Please hold landing' --kind hold --topic ci-hold
  agent-teams check --strict
";

struct Globals {
    teams_path: Option<PathBuf>,
    routes_extra: Vec<PathBuf>,
    state: PathBuf,
    config: PathBuf,
    host: String,
    json: bool,
}

impl Globals {
    fn teams_file(&self) -> PathBuf {
        if let Some(path) = &self.teams_path {
            return path.clone();
        }
        if let Ok(path) = std::env::var("AGENT_TEAMS_FILE") {
            return PathBuf::from(path);
        }
        self.config.join("teams.json")
    }
    fn load_teams(&self) -> Result<TeamsFile, DataError> {
        load_teams(&self.teams_file())
    }
    fn load_routes(&self) -> Result<RoutesFile, DataError> {
        load_routes(&route_dirs(&self.config, &self.routes_extra))
    }
    fn links_dir(&self) -> PathBuf {
        self.state.join("links")
    }
}

fn err(code: &'static str, message: impl Into<String>) -> DataError {
    DataError::new(code, message)
}

/// Run the CLI; returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    match run_inner(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("agent-teams: {e}");
            match e.code {
                "usage" => 2,
                "refused-repeat" | "refused-conflict" => 65,
                _ => 2,
            }
        }
    }
}

fn run_inner(args: &[String]) -> Result<i32, DataError> {
    let mut globals = Globals {
        teams_path: None,
        routes_extra: Vec::new(),
        state: crate::proof::state_dir(),
        config: crate::proof::config_dir(),
        host: crate::proof::this_host(),
        json: false,
    };
    let mut rest: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--teams" => {
                globals.teams_path = Some(PathBuf::from(next(args, &mut i, "--teams")?));
            }
            "--routes" => {
                globals
                    .routes_extra
                    .push(PathBuf::from(next(args, &mut i, "--routes")?));
            }
            "--state-dir" => {
                globals.state = PathBuf::from(next(args, &mut i, "--state-dir")?);
            }
            "--json" => globals.json = true,
            "--help" | "-h" if rest.is_empty() => {
                print!("{HELP}");
                return Ok(0);
            }
            "--version" => {
                println!("agent-teams {}", env!("CARGO_PKG_VERSION"));
                return Ok(0);
            }
            other => rest.push(other.to_string()),
        }
        i += 1;
    }
    let (command, cmd_args) = match rest.split_first() {
        Some((c, a)) => (c.as_str(), a),
        None => {
            print!("{HELP}");
            return Ok(2);
        }
    };
    match command {
        "quickstart" => Ok(print_text(QUICKSTART)),
        "userguide" => Ok(print_text(USERGUIDE)),
        "help" => Ok(print_text(HELP)),
        "show" => cmd_show(&globals, cmd_args),
        "check" => cmd_check(&globals, cmd_args),
        "probe" => cmd_probe(&globals),
        "import" => cmd_import(&globals, cmd_args),
        "init" => cmd_init(&globals, cmd_args),
        "send" => cmd_send(&globals, cmd_args),
        "reply" => cmd_reply(&globals, cmd_args),
        "retract" => cmd_retract(&globals, cmd_args),
        "status" => cmd_status(&globals, cmd_args),
        "outbox" => cmd_outbox(&globals),
        "pump" => cmd_pump(&globals, cmd_args),
        "seen" => cmd_seen(&globals, cmd_args),
        "resolve" => cmd_resolve(&globals, cmd_args),
        "hook" => cmd_hook(&globals, cmd_args),
        "serve" => cmd_serve(&globals, cmd_args),
        "drain-one" => cmd_drain_one(&globals, cmd_args),
        "accept" => cmd_accept(&globals, cmd_args),
        "receipt" => cmd_receipt(&globals, cmd_args),
        "fence" => cmd_fence(&globals, cmd_args),
        "ssh" => cmd_ssh(&globals, cmd_args),
        "link" => cmd_link(&globals, cmd_args),
        other => Err(err(
            "usage",
            format!("unknown command {other:?}; see agent-teams help"),
        )),
    }
}

fn print_text(text: &str) -> i32 {
    print!("{text}");
    if !text.ends_with('\n') {
        println!();
    }
    0
}

fn next(args: &[String], i: &mut usize, flag: &str) -> Result<String, DataError> {
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| err("usage", format!("{flag} needs a value")))
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

fn flag_values(args: &[String], flag: &str) -> Vec<String> {
    args.windows(2)
        .filter(|w| w[0] == flag)
        .map(|w| w[1].clone())
        .collect()
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn positional(args: &[String]) -> Vec<String> {
    let value_flags = [
        "--file",
        "--text",
        "--key",
        "--topic",
        "--supersedes",
        "--ttl",
        "--expires",
        "--kind",
        "--from-service",
        "--from-team",
        "--wait",
        "--team",
        "--registry",
        "--deliver",
        "--accepts-from",
        "--accepts-relay-from",
        "--coordinator-pid",
        "--coordinators-file",
        "--peer",
        "--not-after",
        "--envelope",
        "--envelope-base64",
        "--protocol",
        "--argv",
        "--from",
        "--via",
        "--peer-sender",
        "--route",
        "--in-reply-to",
        "--jump-to",
        "--queue-dir",
        "--ready-timeout",
        "--id",
        "--agent",
    ];
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if value_flags.contains(&args[i].as_str()) {
            i += 2;
        } else if args[i].starts_with("--") {
            i += 1;
        } else {
            out.push(args[i].clone());
            i += 1;
        }
    }
    out
}

fn parse_target(text: &str) -> (String, Option<String>) {
    match text.split_once('@') {
        Some((team, host)) => (team.to_string(), Some(host.to_string())),
        None => (text.to_string(), None),
    }
}

fn reviewed_age(team: &crate::model::Team) -> String {
    let Some(at) = team
        .reviewed
        .as_ref()
        .and_then(|r| r.get("at"))
        .and_then(Value::as_str)
    else {
        return "never reviewed (stale?)".to_string();
    };
    match parse_rfc3339_ms(at) {
        Some(ms) => {
            let age_days = (now_unix_ms() - ms) / 86_400_000;
            if age_days > 7 {
                format!("reviewed {at} ({age_days}d ago, stale?)")
            } else {
                format!("reviewed {at}")
            }
        }
        None => format!("reviewed {at}"),
    }
}

fn cmd_show(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let teams = globals.load_teams()?;
    let routes = globals.load_routes()?;
    let wanted = positional(args).into_iter().next();
    if globals.json || has_flag(args, "--json") {
        let mut value = teams.raw.clone();
        value["route_names"] = json!(routes.routes.keys().collect::<Vec<_>>());
        value["warnings"] = json!(teams.warnings);
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(0);
    }
    for warning in teams.warnings.iter().chain(routes.warnings.iter()) {
        println!("warning: {warning}");
    }
    if !teams.hubs.is_empty() {
        println!("topology: star, hubs {}", teams.hubs.join(", "));
    }
    for (name, host) in &teams.hosts {
        println!(
            "host {name}: routes [{}]{}",
            host.routes.join(", "),
            host.agent_teams
                .as_ref()
                .map(|p| format!(" agent-teams={p}"))
                .unwrap_or_default()
        );
        for (svc, unit) in &host.services {
            println!("  service {svc}: unit {unit}");
        }
    }
    for (name, team) in &teams.teams {
        if wanted.as_deref().is_some_and(|w| w != name) {
            continue;
        }
        println!(
            "team {name}{}: {}",
            team.description
                .as_ref()
                .map(|d| format!(" ({d})"))
                .unwrap_or_default(),
            reviewed_age(team)
        );
        for entry in &team.entries {
            let contacts: Vec<String> = entry.contacts.iter().map(|c| c.label()).collect();
            println!(
                "  @{}{}: contacts [{}]",
                entry.host,
                entry
                    .coordinator
                    .as_ref()
                    .map(|c| format!(" coordinator={c} (informational)"))
                    .unwrap_or_default(),
                contacts.join(", ")
            );
        }
    }
    println!(
        "routes: {}",
        routes.routes.keys().cloned().collect::<Vec<_>>().join(", ")
    );
    Ok(0)
}

/// Warn when host state sits under a config tree: some setups sync
/// `~/.config` between hosts (live-trial finding, 2026-10-25), and a synced
/// ledger, outbox or declaration would break exactly-once. The teams file
/// and public routes may be shared; host state may not.
fn synced_state_warning(state: &Path, config: &Path) -> Option<String> {
    let under_config = state.starts_with(config)
        || state
            .components()
            .any(|c| c.as_os_str() == std::ffi::OsStr::new(".config"));
    if under_config {
        Some(format!(
            "state directory {} is under a config tree; if that tree syncs              between hosts, ledgers/outboxes/declarations would be shared              and exactly-once would break - move state under              $XDG_STATE_HOME/agent-teams (default ~/.local/state/agent-teams)",
            state.display()
        ))
    } else {
        None
    }
}

fn cmd_check(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let teams = globals.load_teams()?;
    let routes = globals.load_routes()?;
    let strict = has_flag(args, "--strict");
    let targets = positional(args);
    let declarations = load_declarations(&globals.state).unwrap_or_default();
    let mut problems: Vec<String> = Vec::new();
    let mut lines: Vec<Value> = Vec::new();
    for warning in teams.warnings.iter().chain(routes.warnings.iter()) {
        if strict {
            problems.push(warning.clone());
        }
        lines.push(json!({"kind": "warning", "detail": warning}));
    }
    if let Some(warning) = synced_state_warning(&globals.state, &globals.config) {
        if strict {
            problems.push(warning.clone());
        }
        lines.push(json!({"kind": "warning", "detail": warning}));
    }
    // Hosts must name routes that exist.
    for (name, host) in &teams.hosts {
        for route_name in &host.routes {
            if !routes.routes.contains_key(route_name) {
                problems.push(format!("host {name}: route {route_name:?} is not defined"));
            }
        }
        let remote = name != &globals.host;
        if remote && host.agent_teams.is_none() && !host.routes.is_empty() {
            lines.push(json!({"kind": "warning", "detail": format!("host {name}: no agent_teams path; remote routes need one")}));
        }
    }
    // Star: every spoke needs a route toward the first hub.
    if let Some(hub) = teams.hubs.first() {
        if globals.host != *hub {
            match teams.hosts.get(hub) {
                Some(hub_host) if !hub_host.routes.is_empty() => {
                    lines.push(json!({"kind": "topology", "detail": format!("hub {hub} routes [{}]", hub_host.routes.join(", "))}));
                }
                _ => problems.push(format!("no-hub-link: no route from this host to hub {hub}")),
            }
        }
    }
    // Entries: local ones need a live declaration; remote ones a route.
    for (name, team) in &teams.teams {
        if !targets.is_empty()
            && !targets
                .iter()
                .any(|t| t == name || t.starts_with(&format!("{name}@")))
        {
            continue;
        }
        for entry in &team.entries {
            if entry.host == globals.host {
                match declarations.iter().find(|(t, _, _)| t == name) {
                    Some((_, _, declaration)) => {
                        let registry = declaration
                            .deliverer
                            .get("registry")
                            .and_then(Value::as_str)
                            .map(crate::jsonfile::expand_home);
                        let live =
                            coordinator_identity(&declaration.designation, registry.as_deref())
                                .is_some_and(|id| identity_alive(&id));
                        if !live {
                            problems.push(format!(
                                "team {name}@{}: designated coordinator is not live",
                                entry.host
                            ));
                        } else {
                            lines.push(json!({"kind": "ok", "detail": format!("team {name}@{}: declaration live ({})", entry.host, declaration.deliverer_kind)}));
                        }
                    }
                    None => problems.push(format!(
                        "team {name}@{}: no declaration on this host; run agent-teams init {name}",
                        entry.host
                    )),
                }
            } else {
                match teams.hosts.get(&entry.host) {
                    Some(host) if !host.routes.is_empty() => {
                        lines.push(json!({"kind": "ok", "detail": format!("team {name}@{}: routes [{}] (not probed)", entry.host, host.routes.join(", "))}));
                    }
                    _ => problems.push(format!("team {name}@{}: no-route", entry.host)),
                }
            }
        }
    }
    if globals.json || has_flag(args, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ok": problems.is_empty(), "problems": problems, "lines": lines,
            }))?
        );
    } else {
        for line in &lines {
            println!(
                "{}: {}",
                line["kind"].as_str().unwrap_or("?"),
                line["detail"].as_str().unwrap_or("")
            );
        }
        for problem in &problems {
            println!("problem: {problem}");
        }
        println!(
            "{}",
            if problems.is_empty() {
                "check: OK"
            } else {
                "check: FAILED"
            }
        );
    }
    Ok(if problems.is_empty() { 0 } else { 1 })
}

fn cmd_probe(globals: &Globals) -> Result<i32, DataError> {
    let value = probe(&globals.state, &globals.host);
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(0)
}

fn cmd_import(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let file = flag_value(args, "--coordinators-file")
        .ok_or_else(|| err("usage", "import needs --coordinators-file FILE"))?;
    let team =
        flag_value(args, "--team").ok_or_else(|| err("usage", "import needs --team NAME"))?;
    if !crate::model::valid_team_name(&team) {
        return Err(err(
            "usage",
            format!("team name {team:?} must match ^[a-z0-9-]{{1,32}}$"),
        ));
    }
    let source = read_json(Path::new(&file))?;
    let hosts_in = source
        .get("hosts")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut hosts_out = serde_json::Map::new();
    let mut entries = Vec::new();
    let mut routes_out = serde_json::Map::new();
    let mut notes = Vec::new();
    for (short, host_value) in &hosts_in {
        let fqdn = host_value
            .get("fqdn")
            .and_then(Value::as_str)
            .unwrap_or(short.as_str());
        let route_name = format!("ssh-{short}");
        let mut host_routes = vec![route_name.clone()];
        if *short == globals.host {
            host_routes = vec!["local".to_string()];
        } else {
            routes_out.insert(
                route_name,
                json!({
                    "driver": "ssh",
                    "target": fqdn,
                    "use_user_ssh_config": true,
                    "comment": "imported; dedicated keys stay optional (design 10.3)",
                }),
            );
        }
        let coordinator = host_value
            .get("coordinator")
            .cloned()
            .unwrap_or(Value::Null);
        let mut contacts = vec![json!({"kind": "host"})];
        if let Some(list) = coordinator.get("contacts").and_then(Value::as_array) {
            for contact in list {
                match contact.get("kind").and_then(Value::as_str) {
                    Some("agentcloud") => {
                        contacts.push(json!({
                            "kind": "session",
                            "route": "cloud-session",
                            "session": contact.get("session"),
                            "comment": "imported session contact; define the cloud-session route in a private routes file",
                        }));
                    }
                    Some("herdr") => notes.push(format!("host {short}: herdr contact dropped (never mapped)")),
                    Some("agentctl") => notes.push(format!(
                        "host {short}: agentctl registry {:?} belongs in that host's declaration (deliverer.registry)",
                        contact.get("registry").and_then(Value::as_str).unwrap_or("?")
                    )),
                    other => notes.push(format!("host {short}: contact kind {other:?} not mapped")),
                }
            }
        }
        hosts_out.insert(
            short.clone(),
            json!({"routes": host_routes, "agent_teams": "~/.local/bin/agent-teams"}),
        );
        entries.push(json!({
            "host": short,
            "coordinator": coordinator.get("name"),
            "contacts": contacts,
        }));
    }
    let proposal = json!({
        "teams.json": {
            "schema": "agent-teams-teams/v1",
            "comment": "Printed by agent-teams import; review before use. Nothing was written.",
            "hosts": hosts_out,
            "teams": { team.clone(): {"entries": entries} },
        },
        "routes.d/imported.json": {
            "schema": "agent-teams-routes/v1",
            "routes": routes_out,
        },
        "notes": notes,
    });
    println!("{}", serde_json::to_string_pretty(&proposal)?);
    Ok(0)
}

fn declaration_path(globals: &Globals, team: &str) -> PathBuf {
    globals
        .state
        .join("teams.d")
        .join(team)
        .join("declaration.json")
}

fn cmd_init(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let team = positional(args)
        .into_iter()
        .next()
        .ok_or_else(|| err("usage", "init needs a TEAM name"))?;
    if !crate::model::valid_team_name(&team) {
        return Err(err(
            "usage",
            format!("team name {team:?} must match ^[a-z0-9-]{{1,32}}$"),
        ));
    }
    let registry = flag_value(args, "--registry");
    let deliver = flag_value(args, "--deliver").unwrap_or_else(|| {
        if registry.is_some() {
            "agentctl-queue".to_string()
        } else {
            "hook-inbox".to_string()
        }
    });
    let (coordinator, deliverer) = if let Some(registry) = &registry {
        (
            json!({"designation": "registry", "comment": "the registry's own coordinator designation; agentctl keeps it current"}),
            json!({"kind": deliver, "registry": crate::jsonfile::expand_home(registry).to_string_lossy()}),
        )
    } else if has_flag(args, "--coordinator-self") {
        let pid: u64 = match flag_value(args, "--coordinator-pid") {
            Some(text) => text
                .parse()
                .map_err(|_| err("usage", "--coordinator-pid must be a process ID"))?,
            None => crate::proof::ancestry()
                .get(1)
                .copied()
                .unwrap_or_else(|| u64::from(std::process::id())),
        };
        let identity =
            identity_of(pid).ok_or_else(|| err("usage", format!("process {pid} is not live")))?;
        let mut deliverer = json!({"kind": deliver});
        if deliver == "command" {
            let argv: Vec<String> = args
                .iter()
                .skip_while(|a| *a != "--argv")
                .skip(1)
                .cloned()
                .collect();
            deliverer["argv"] = json!(argv);
        }
        (
            json!({
                "designation": "pinned",
                "harness_identity": serde_json::to_value(&identity)?,
                "comment": "refresh by re-running init --coordinator-self from the new session",
            }),
            deliverer,
        )
    } else {
        return Err(err(
            "usage",
            "init needs --registry R or --coordinator-self",
        ));
    };
    let accepts: Vec<Value> = flag_values(args, "--accepts-from")
        .iter()
        .map(|sender| json!({"sender": sender, "kinds": ["info", "request", "hold", "resume", "reply"]}))
        .collect();
    let mut declaration = json!({
        "schema": "agent-teams-declaration/v1",
        "team": team,
        "deliverer": deliverer,
        "coordinator": coordinator,
        "accepts_from": accepts,
        "may_send_to": [],
        "limits": {"max_bytes": 8192, "per_sender_per_hour": 30, "max_ttl_seconds": 604800},
        "default_ttl_seconds": 7200,
        "accepts_relay_from": flag_values(args, "--accepts-relay-from"),
        "updated_at": format_rfc3339_ms(now_unix_ms()),
        "updated_by": "agent-teams init",
    });
    let path = declaration_path(globals, &team);
    // Preserve comments and policy of an existing declaration on re-init.
    if let Ok(existing) = load_declaration(&path) {
        if declaration["accepts_from"]
            .as_array()
            .is_some_and(|a| a.is_empty())
        {
            declaration["accepts_from"] = existing.raw["accepts_from"].clone();
        }
        for key in ["comment", "notes", "may_send_to"] {
            if existing.raw.get(key).is_some() && !existing.raw[key].is_null() {
                declaration[key] = existing.raw[key].clone();
            }
        }
    }
    write_json_atomic(&path, &declaration)?;
    println!(
        "declared team {team} on {} ({})",
        globals.host,
        path.display()
    );
    if deliver == "hook-inbox" {
        println!("hook-inbox never wakes an idle agent. Add this harness hook:");
        println!("  command: agent-teams hook --team {team}   (UserPromptSubmit or Stop)");
    }
    Ok(0)
}

/// Prove the sender for `send`/`reply`: the root coordinator of a declared
/// team on this host, or a registered service. Lieutenants are refused.
fn prove_sender(
    globals: &Globals,
    teams: &TeamsFile,
    args: &[String],
) -> Result<(String, String, Declaration), DataError> {
    if let Some(service) = flag_value(args, "--from-service") {
        let host = teams.hosts.get(&globals.host);
        let unit = host.and_then(|h| h.services.get(&service)).ok_or_else(|| {
            err(
                "refused",
                format!(
                    "refused: sender-not-service (no service {service:?} registered for host {})",
                    globals.host
                ),
            )
        })?;
        // A worker refused as itself cannot retry as a service.
        for (_, _, declaration) in load_declarations(&globals.state).unwrap_or_default() {
            if let Some(registry) = declaration
                .deliverer
                .get("registry")
                .and_then(Value::as_str)
                .map(crate::jsonfile::expand_home)
            {
                if ancestry_has_agent(&registry) {
                    return Err(err(
                        "refused",
                        "refused: sender-is-agent (an agent harness is in the ancestry; services are not agents)",
                    ));
                }
            }
        }
        if !inside_unit(unit) {
            return Err(err(
                "refused",
                format!("refused: sender-not-service (this process is not inside unit {unit})"),
            ));
        }
        let declarations = load_declarations(&globals.state).unwrap_or_default();
        let own = declarations
            .into_iter()
            .next()
            .map(|(_, _, d)| d)
            .ok_or_else(|| err("refused", "refused: no declaration on this host"))?;
        return Ok((
            format!("service:{service}"),
            "service-unit".to_string(),
            own,
        ));
    }
    let wanted = flag_value(args, "--from-team");
    let declarations = load_declarations(&globals.state)?;
    let mut refused: Option<String> = None;
    for (team, _, declaration) in declarations {
        if wanted.as_deref().is_some_and(|w| w != team) {
            continue;
        }
        let registry = declaration
            .deliverer
            .get("registry")
            .and_then(Value::as_str)
            .map(crate::jsonfile::expand_home);
        match sender_standing(&declaration.designation, registry.as_deref()) {
            SenderStanding::Coordinator => {
                return Ok((team, "harness-ancestry".to_string(), declaration));
            }
            SenderStanding::NotCoordinator { agent, lieutenant } => {
                refused = Some(format!(
                    "refused: sender-not-coordinator (you are {agent}{}; ask your coordinator)",
                    if lieutenant { ", a lieutenant" } else { "" }
                ));
            }
            SenderStanding::Unknown => {}
        }
    }
    Err(err(
        "refused",
        refused.unwrap_or_else(|| {
            "refused: sender-not-coordinator (no declared team's coordinator is in this process ancestry; ask your coordinator)".to_string()
        }),
    ))
}

fn read_body(args: &[String]) -> Result<String, DataError> {
    if let Some(text) = flag_value(args, "--text") {
        return Ok(text);
    }
    if let Some(file) = flag_value(args, "--file") {
        if file == "-" {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|e| err("io", e.to_string()))?;
            return Ok(text);
        }
        return std::fs::read_to_string(&file).map_err(|e| err("io", format!("{file}: {e}")));
    }
    Err(err("usage", "needs --file FILE or --text TEXT"))
}

fn routes_for_host(
    globals: &Globals,
    teams: &TeamsFile,
    routes: &RoutesFile,
    host_name: &str,
) -> Result<Vec<Box<dyn Route>>, DataError> {
    let host = teams.hosts.get(host_name).ok_or_else(|| {
        err(
            "unknown-host",
            format!("no host {host_name:?} in teams.json"),
        )
    })?;
    let remote_binary = host
        .agent_teams
        .clone()
        .unwrap_or_else(|| "agent-teams".to_string());
    let mut out = Vec::new();
    for name in &host.routes {
        let def = routes
            .routes
            .get(name)
            .ok_or_else(|| err("unknown-route", format!("route {name:?} is not defined")))?;
        out.push(route_for(
            def,
            &globals.host,
            host_name,
            &remote_binary,
            &globals.links_dir(),
        )?);
    }
    Ok(out)
}

fn cmd_send(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let target = positional(args)
        .into_iter()
        .next()
        .ok_or_else(|| err("usage", "send needs TEAM[@HOST]"))?;
    let (to_team, to_host_arg) = parse_target(&target);
    let body = read_body(args)?;
    let teams = globals.load_teams()?;
    let routes = globals.load_routes()?;
    let (sender, proof_kind, own) = prove_sender(globals, &teams, args)?;
    if !own.may_send_to.is_empty() && !own.may_send_to.iter().any(|t| t == &to_team) {
        return Err(err(
            "refused",
            format!(
                "refused: may-send-to (team {} may not send to {to_team})",
                own.team
            ),
        ));
    }
    let from_team = sender
        .strip_prefix("service:")
        .map(|name| format!("svc.{name}"))
        .unwrap_or_else(|| sender.clone());
    let entry = teams
        .pick_entry(&to_team, to_host_arg.as_deref(), &globals.host)?
        .clone();
    let to_host = entry.host.clone();
    let addressed_to = if to_host_arg.is_some() || entry.host != globals.host {
        format!("{to_team}@{to_host}")
    } else {
        to_team.clone()
    };
    let kind = flag_value(args, "--kind").unwrap_or_else(|| "info".to_string());
    if !["info", "request", "hold", "resume", "reply"].contains(&kind.as_str()) {
        return Err(err("usage", format!("unknown kind {kind:?}")));
    }
    let topic = flag_value(args, "--topic");
    let mut key =
        flag_value(args, "--key").unwrap_or_else(|| derived_key(&kind, topic.as_deref(), &body));
    let key_was_derived = flag_value(args, "--key").is_none();
    if has_flag(args, "--again") {
        key = format!("{key}~{}", now_unix());
    }
    let id = message_id(&sender, &globals.host, &addressed_to, &key);
    // Same key, same body is a retry; different body is a conflict.
    if let Some(existing) = find_by_key(&globals.state, &addressed_to, &key) {
        let existing_body = existing["envelope"]["body"].as_str().unwrap_or_default();
        if existing_body != body {
            return Err(err(
                "refused-conflict",
                format!("refused: conflict (key {key:?} already sent {id} with a different body; use a new --key that supersedes it)"),
            ));
        }
        let state = existing["state"].as_str().unwrap_or("deferred").to_string();
        // A repeat of a settled message under a derived key is refused, so a
        // second identical hold is never silently swallowed (section 8.1).
        if key_was_derived && is_terminal(&state) && !has_flag(args, "--again") {
            return Err(err(
                "refused-repeat",
                format!("refused: repeat (already {state} as {id}; pass --key or --again to send it again)"),
            ));
        }
        println!("{id} {state} (retry of the same key)");
        return Ok(exit_code_for(&state));
    }
    let now_ms = now_unix_ms();
    let default_ttl = load_declarations(&globals.state)
        .ok()
        .and_then(|ds| ds.into_iter().find(|(t, _, _)| *t == sender))
        .map(|(_, _, d)| d.default_ttl_seconds)
        .unwrap_or(7200);
    let not_after_ms = if let Some(expires) = flag_value(args, "--expires") {
        parse_rfc3339_ms(&expires).ok_or_else(|| {
            err(
                "usage",
                format!("--expires {expires:?} is not an RFC 3339 time"),
            )
        })?
    } else if let Some(ttl) = flag_value(args, "--ttl") {
        let secs = parse_duration_seconds(&ttl)
            .ok_or_else(|| err("usage", format!("--ttl {ttl:?} is not a duration")))?;
        now_ms + secs * 1000
    } else {
        now_ms + default_ttl * 1000
    };
    let envelope = build_envelope(
        &id,
        &key,
        &from_team,
        &globals.host,
        next_seq(&globals.state),
        &proof_kind,
        &to_team,
        &to_host,
        &kind,
        topic.as_deref(),
        &flag_values(args, "--supersedes"),
        has_flag(args, "--private"),
        now_ms,
        not_after_ms,
        flag_value(args, "--in-reply-to").as_deref(),
        &body,
    );
    let proof = json!({
        "sender": sender, "proof": proof_kind, "host": globals.host,
        "pid": std::process::id(), "at": format_rfc3339_ms(now_ms),
    });
    let mut outbox_entry = write_outbox(&globals.state, &envelope, &addressed_to, &proof)?;
    let host_obj = teams
        .hosts
        .get(&to_host)
        .ok_or_else(|| err("unknown-host", format!("no host {to_host:?} in teams.json")))?
        .clone();
    let route_objs = routes_for_host(globals, &teams, &routes, &to_host)?;
    let result = crate::sender::walk(
        &globals.state,
        &mut outbox_entry,
        &entry,
        &host_obj,
        &route_objs,
    )?;
    log_event(
        &globals.state,
        &json!({
            "event": "send", "id": id, "key": key, "sender": sender,
            "to": addressed_to, "state": result.state, "tag": team_tag(&envelope),
        }),
    );
    if globals.json || has_flag(args, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "id": id, "key": key, "state": result.state, "tag": team_tag(&envelope),
                "steps": result.steps.iter().map(|s| json!({"contact": s.contact, "route": s.route, "detail": s.detail})).collect::<Vec<_>>(),
            }))?
        );
    } else {
        println!("{}", team_tag(&envelope));
        for step in &result.steps {
            println!("  {} {} {}", step.contact, step.route, step.detail);
        }
        println!("{id}: {}", result.state);
    }
    Ok(result.exit_code)
}

fn find_ledger_entry_any_team(globals: &Globals, id: &str) -> Option<(String, Value)> {
    for (team, _, _) in load_declarations(&globals.state).ok()? {
        if let Some(entry) = read_entry(&globals.state, &team, id) {
            return Some((team, entry));
        }
    }
    // Also search ledger directories of undeclared teams (an old declaration).
    let dir = globals.state.join("teams.d");
    for team_dir in std::fs::read_dir(dir).ok()?.flatten() {
        let team = team_dir.file_name().to_string_lossy().into_owned();
        if let Some(entry) = read_entry(&globals.state, &team, id) {
            return Some((team, entry));
        }
    }
    None
}

fn cmd_reply(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let id = positional(args)
        .into_iter()
        .next()
        .ok_or_else(|| err("usage", "reply needs an ID"))?;
    let (team, entry) = find_ledger_entry_any_team(globals, &id)
        .ok_or_else(|| err("unknown", format!("no message {id} in this host's ledgers")))?;
    let original = entry.get("envelope").cloned().ok_or_else(|| {
        err(
            "unknown",
            format!("ledger entry {id} holds no envelope to reply to"),
        )
    })?;
    let to_team = original["reply_to"]["team"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let to_host = original["reply_to"]["host"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let _ = team;
    let mut send_args: Vec<String> = vec![format!("{to_team}@{to_host}")];
    if let Some(file) = flag_value(args, "--file") {
        send_args.extend(["--file".into(), file]);
    }
    if let Some(text) = flag_value(args, "--text") {
        send_args.extend(["--text".into(), text]);
    }
    send_args.extend(["--kind".into(), "reply".into()]);
    let key = flag_value(args, "--key").unwrap_or_else(|| format!("reply:{id}"));
    send_args.extend(["--key".into(), key, "--in-reply-to".into(), id.clone()]);
    if let Some(from_team) = flag_value(args, "--from-team") {
        send_args.extend(["--from-team".into(), from_team]);
    }
    if has_flag(args, "--json") {
        send_args.push("--json".into());
    }
    cmd_send(globals, &send_args)
}

fn cmd_retract(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let id = positional(args)
        .into_iter()
        .next()
        .ok_or_else(|| err("usage", "retract needs an ID"))?;
    let entry = read_outbox(&globals.state, &id)
        .ok_or_else(|| err("unknown", format!("no outbox entry {id}")))?;
    let envelope = &entry["envelope"];
    let to_team = envelope["to"]["team"].as_str().unwrap_or_default();
    let to_host = envelope["to"]["host"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if to_host == globals.host {
        let declarations = load_declarations(&globals.state)?;
        let (_, _, declaration) = declarations
            .iter()
            .find(|(t, _, _)| t == to_team)
            .ok_or_else(|| err("unknown", format!("no declaration for team {to_team}")))?;
        let ledger_path = crate::ledger::entry_path(&globals.state, to_team, &id);
        let deliverer = deliverer_for(
            declaration,
            &globals.state.join("teams.d").join(to_team),
            &ledger_path,
        )?;
        let answer = retract(&globals.state, to_team, &id, deliverer.as_ref())?;
        println!("{id}: {}", answer["outcome"].as_str().unwrap_or("?"));
        return Ok(if answer["outcome"] == "retracted" {
            65
        } else {
            1
        });
    }
    // Remote retract rides a fence-style verb: ask, then report. A tombstone
    // verb is not in protocol 1, so retract over a route is refused here.
    Err(err(
        "usage",
        format!("retract to host {to_host} needs a route call; run it again when pump can reach that host (protocol 1 has no retract verb)"),
    ))
}

fn cmd_status(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let id = positional(args)
        .into_iter()
        .next()
        .ok_or_else(|| err("usage", "status needs an ID"))?;
    let outbox = read_outbox(&globals.state, &id);
    let ledger = find_ledger_entry_any_team(globals, &id);
    if globals.json || has_flag(args, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "id": id,
                "outbox": outbox,
                "ledger": ledger.map(|(team, entry)| json!({"team": team, "entry": entry})),
            }))?
        );
        let state = outbox
            .as_ref()
            .and_then(|o| o["state"].as_str().map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string());
        return Ok(exit_code_for(&state));
    }
    match (&outbox, &ledger) {
        (Some(entry), _) => {
            println!(
                "{id}: {} -> {} ({})",
                entry["state"].as_str().unwrap_or("?"),
                entry["addressed_to"].as_str().unwrap_or("?"),
                entry["envelope"]["key"].as_str().unwrap_or("?")
            );
            if let Some(attempts) = entry["attempts"].as_array() {
                for attempt in attempts {
                    println!(
                        "  {} {} {} {}",
                        attempt["at"].as_str().unwrap_or("?"),
                        attempt["route"].as_str().unwrap_or("?"),
                        attempt["verb"].as_str().unwrap_or("?"),
                        attempt["classification"].as_str().unwrap_or("?")
                    );
                }
            }
            Ok(exit_code_for(entry["state"].as_str().unwrap_or("deferred")))
        }
        (None, Some((team, entry))) => {
            println!(
                "{id}: {} in team {team}'s ledger",
                entry["state"].as_str().unwrap_or("?")
            );
            Ok(exit_code_for(entry["state"].as_str().unwrap_or("deferred")))
        }
        (None, None) => {
            println!("{id}: unknown here");
            Ok(76)
        }
    }
}

fn cmd_outbox(globals: &Globals) -> Result<i32, DataError> {
    let dir = crate::sender::outbox_dir(&globals.state);
    let mut rows = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if !crate::ledger::is_entry_file_name(&entry.file_name().to_string_lossy()) {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    rows.push(value);
                }
            }
        }
    }
    rows.sort_by_key(|row| row["id"].as_str().unwrap_or_default().to_string());
    if globals.json {
        println!("{}", serde_json::to_string_pretty(&json!(rows))?);
        return Ok(0);
    }
    if rows.is_empty() {
        println!("outbox is empty");
    }
    for row in &rows {
        println!(
            "{} {} -> {} key={}",
            row["id"].as_str().unwrap_or("?"),
            row["state"].as_str().unwrap_or("?"),
            row["addressed_to"].as_str().unwrap_or("?"),
            row["envelope"]["key"].as_str().unwrap_or("?")
        );
    }
    Ok(0)
}

fn cmd_pump(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let retry_refused = has_flag(args, "--retry-refused");
    let teams = globals.load_teams()?;
    let routes = globals.load_routes()?;
    let dir = crate::sender::outbox_dir(&globals.state);
    let mut worst = 0;
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for file in entries.flatten() {
            if !crate::ledger::is_entry_file_name(&file.file_name().to_string_lossy()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(file.path()) else {
                continue;
            };
            let Ok(mut entry) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let state = entry["state"].as_str().unwrap_or("deferred").to_string();
            let retryable = matches!(
                state.as_str(),
                "deferred" | "ambiguous" | "accepted" | "claimed"
            ) || (retry_refused && state == "refused");
            if !retryable {
                continue;
            }
            if retry_refused && state == "refused" {
                // A successor under a new key, never the refused ID (9.4).
                println!(
                    "{}: refused; send a successor with --key <key>~2 (pump never reopens a refused ID)",
                    entry["id"].as_str().unwrap_or("?")
                );
                continue;
            }
            let (to_team, to_host_arg) =
                parse_target(entry["addressed_to"].as_str().unwrap_or_default());
            let Ok(team_entry) = teams.pick_entry(&to_team, to_host_arg.as_deref(), &globals.host)
            else {
                continue;
            };
            let team_entry = team_entry.clone();
            let Some(host_obj) = teams.hosts.get(&team_entry.host).cloned() else {
                continue;
            };
            let route_objs = routes_for_host(globals, &teams, &routes, &team_entry.host)?;
            let result = crate::sender::walk(
                &globals.state,
                &mut entry,
                &team_entry,
                &host_obj,
                &route_objs,
            )?;
            println!("{}: {}", entry["id"].as_str().unwrap_or("?"), result.state);
            worst = worst.max(result.exit_code);
            count += 1;
        }
    }
    if count == 0 {
        println!("pump: nothing to do");
    }
    Ok(worst)
}

fn cmd_seen(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let id = positional(args)
        .into_iter()
        .next()
        .ok_or_else(|| err("usage", "seen needs an ID"))?;
    match find_ledger_entry_any_team(globals, &id) {
        Some((team, entry)) => {
            println!(
                "{id}: {} in team {team}'s ledger{}",
                entry["state"].as_str().unwrap_or("?"),
                entry
                    .get("recorded_at")
                    .and_then(Value::as_str)
                    .map(|at| format!(" since {at}"))
                    .unwrap_or_default()
            );
            Ok(0)
        }
        None => {
            println!("{id}: not seen here");
            Ok(1)
        }
    }
}

fn cmd_resolve(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let id = positional(args)
        .into_iter()
        .next()
        .ok_or_else(|| err("usage", "resolve needs an ID"))?;
    let team_arg = flag_value(args, "--team");
    let (team, mut entry) = match &team_arg {
        Some(team) => (
            team.clone(),
            read_entry(&globals.state, team, &id)
                .ok_or_else(|| err("unknown", format!("no entry {id} for team {team}")))?,
        ),
        None => find_ledger_entry_any_team(globals, &id)
            .ok_or_else(|| err("unknown", format!("no ledger entry {id} here")))?,
    };
    if has_flag(args, "--mark-delivered") {
        entry["state"] = json!("delivered");
        entry["detail"] = json!("marked delivered by a human (resolve)");
    } else if has_flag(args, "--fence") {
        entry["state"] = json!("fenced");
        entry["detail"] = json!("fenced by a human (resolve)");
    } else {
        return Err(err("usage", "resolve needs --fence or --mark-delivered"));
    }
    entry["recorded_at"] = json!(format_rfc3339_ms(now_unix_ms()));
    write_json_atomic(
        &crate::ledger::entry_path(&globals.state, &team, &id),
        &entry,
    )?;
    println!("{id}: {}", entry["state"].as_str().unwrap_or("?"));
    Ok(0)
}

fn cmd_hook(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    // The hook claims items by rename, re-checks expiry and tombstones, and
    // prints what survives (section 9.5). A later run in the same harness
    // upgrades `presented` to `delivered`.
    let wanted = flag_value(args, "--team");
    let declarations = load_declarations(&globals.state)?;
    let mut printed_any = false;
    for (team, _, declaration) in &declarations {
        if wanted.as_deref().is_some_and(|w| w != team) {
            continue;
        }
        if declaration.deliverer_kind != "hook-inbox" {
            continue;
        }
        let inbox = declaration
            .deliverer
            .get("inbox_dir")
            .and_then(Value::as_str)
            .map(crate::jsonfile::expand_home)
            .unwrap_or_else(|| globals.state.join("teams.d").join(team).join("hook-inbox"));
        // Upgrade earlier presented items: this run proves the harness took them.
        for mut entry in list_entries(&globals.state, team) {
            if entry["state"] == "presented" {
                entry["state"] = json!("delivered");
                let id = entry["id"].as_str().unwrap_or_default().to_string();
                write_json_atomic(
                    &crate::ledger::entry_path(&globals.state, team, &id),
                    &entry,
                )?;
            }
        }
        let claimed_dir = inbox.join("claimed");
        std::fs::create_dir_all(&claimed_dir)?;
        let Ok(files) = std::fs::read_dir(&inbox) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = files
            .flatten()
            .map(|f| f.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();
        for path in paths {
            let Some(name) = path.file_stem().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            let claimed = claimed_dir.join(format!("{name}.json"));
            if std::fs::rename(&path, &claimed).is_err() {
                continue; // another hook run claimed it
            }
            let Ok(text) = std::fs::read_to_string(&claimed) else {
                continue;
            };
            let Ok(item) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let ledger = read_entry(&globals.state, team, &name);
            let ledger_state = ledger
                .as_ref()
                .map(|e| e["state"].as_str().unwrap_or("?").to_string())
                .unwrap_or_default();
            let tombstoned = inbox.join("cancel").join(format!("{name}.json")).exists();
            let expired = item["not_after"]
                .as_str()
                .and_then(parse_rfc3339_ms)
                .is_some_and(|ms| ms <= now_unix_ms());
            if tombstoned || expired || !matches!(ledger_state.as_str(), "accepted" | "claimed") {
                continue; // moved aside by the claim; never printed twice
            }
            if let Some(mut entry) = ledger {
                entry["state"] = json!("presented");
                write_json_atomic(
                    &crate::ledger::entry_path(&globals.state, team, &name),
                    &entry,
                )?;
            }
            print!("{}", item["text"].as_str().unwrap_or_default());
            printed_any = true;
        }
    }
    let _ = printed_any;
    Ok(0)
}

/// `drain-one`: the detached delivery-on-arrival helper. Spawned by the
/// agentctl-queue deliverer on the receiving host; runs one bounded
/// `agentctl drain` under the per-agent lock. Outcomes land in
/// `team-log.jsonl`, never in message state.
fn cmd_drain_one(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let need = |flag: &str| {
        flag_value(args, flag).ok_or_else(|| err("usage", format!("drain-one needs {flag}")))
    };
    let queue_dir = PathBuf::from(need("--queue-dir")?);
    let registry = PathBuf::from(need("--registry")?);
    let agent = need("--agent")?;
    let id = need("--id")?;
    let ready_timeout: u64 = need("--ready-timeout")?
        .parse()
        .map_err(|_| err("usage", "--ready-timeout must be seconds"))?;
    let state = flag_value(args, "--state-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| globals.state.clone());
    crate::deliver::run_auto_drain(&state, &queue_dir, &registry, &agent, &id, ready_timeout);
    Ok(0)
}

fn cmd_serve(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let peer = flag_value(args, "--peer").unwrap_or_else(|| "unknown".to_string());
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "{}", banner(&globals.host)).map_err(|e| err("io", e.to_string()))?;
    out.flush().map_err(|e| err("io", e.to_string()))?;
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    // A forced command may get its verb from SSH_ORIGINAL_COMMAND; the frame
    // still carries verb and payload, and the frame wins.
    let _ = std::env::var("SSH_ORIGINAL_COMMAND");
    if let Some((verb, payload)) = read_frame(&mut reader) {
        let answer = handle_verb(&globals.state, &globals.host, &peer, &verb, &payload);
        writeln!(out, "{}", answer).map_err(|e| err("io", e.to_string()))?;
        out.flush().map_err(|e| err("io", e.to_string()))?;
    }
    Ok(0)
}

fn cmd_accept(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    if flag_value(args, "--protocol").as_deref() != Some("1") {
        println!(
            "{}",
            json!({"outcome": "route-refused", "detail": "remote-too-old: accept needs --protocol 1"})
        );
        return Ok(0);
    }
    let envelope: Value = if let Some(encoded) = flag_value(args, "--envelope-base64") {
        let bytes = base64_decode(&encoded)
            .ok_or_else(|| err("usage", "--envelope-base64 is not base64"))?;
        serde_json::from_slice(&bytes)?
    } else {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| err("io", e.to_string()))?;
        serde_json::from_str(&text)?
    };
    let team_name = envelope["to"]["team"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let declarations = load_declarations(&globals.state)?;
    let Some((_, _, declaration)) = declarations.iter().find(|(t, _, _)| *t == team_name) else {
        println!(
            "{}",
            json!({"outcome": "route-refused", "detail": format!("no declaration for team {team_name} on this host")})
        );
        return Ok(0);
    };
    let id = envelope["id"].as_str().unwrap_or_default();
    let ledger_path = crate::ledger::entry_path(&globals.state, &team_name, id);
    let deliverer = deliverer_for(
        declaration,
        &globals.state.join("teams.d").join(&team_name),
        &ledger_path,
    )?;
    let proof = envelope["from"]["proof"].as_str().unwrap_or("asserted");
    let ctx = AcceptContext {
        state: &globals.state,
        this_host: &globals.host,
        declaration,
        via: "local",
        proof_note: &format!("sender proved on {} ({proof})", globals.host),
        relayed_by: None,
        peer_host: &globals.host,
    };
    let answer = accept(&ctx, &envelope, deliverer.as_ref())?;
    println!("{}", serde_json::to_string_pretty(&answer.to_json())?);
    Ok(0)
}

fn cmd_receipt(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let id = positional(args)
        .into_iter()
        .next()
        .ok_or_else(|| err("usage", "receipt needs an ID"))?;
    let team = flag_value(args, "--team");
    let value = match &team {
        Some(team) => receipt(&globals.state, team, &id),
        None => match find_ledger_entry_any_team(globals, &id) {
            Some((team, _)) => receipt(&globals.state, &team, &id),
            None => json!({"known": false, "id": id, "state": "unknown"}),
        },
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(exit_code_for(value["state"].as_str().unwrap_or("unknown")))
}

fn cmd_fence(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let id = positional(args)
        .into_iter()
        .next()
        .ok_or_else(|| err("usage", "fence needs an ID"))?;
    let team = flag_value(args, "--team").ok_or_else(|| err("usage", "fence needs --team T"))?;
    let not_after = flag_value(args, "--not-after")
        .unwrap_or_else(|| format_rfc3339_ms(now_unix_ms() + 604_800_000));
    let value = fence(&globals.state, &team, &id, &not_after, None)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(0)
}

fn cmd_ssh(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let verb = positional(args).into_iter().next();
    if verb.as_deref() != Some("enroll") {
        return Err(err("usage", "ssh subcommand: enroll"));
    }
    let peer =
        flag_value(args, "--peer").ok_or_else(|| err("usage", "ssh enroll needs --peer HOST"))?;
    // Keys are per-host secrets: they live under the state directory, never
    // under ~/.config, which some setups sync between hosts.
    let dir = globals.state.join("ssh");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let key = dir.join(format!("{}-to-{}", globals.host, peer));
    if !key.exists() {
        let status = std::process::Command::new("ssh-keygen")
            .args(["-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .arg("-C")
            .arg(format!("agent-teams {}->{}", globals.host, peer))
            .status()
            .map_err(|e| err("io", format!("ssh-keygen failed to start: {e}")))?;
        if !status.success() {
            return Err(err("io", "ssh-keygen failed"));
        }
    }
    let public = std::fs::read_to_string(key.with_extension("pub"))?
        .trim()
        .to_string();
    let binary = std::env::current_exe()?;
    let from = flag_value(args, "--from")
        .map(|addr| format!("from=\"{addr}\","))
        .unwrap_or_default();
    println!("# Add this line to the peer account's authorized_keys on {peer} (agent-teams never edits it):");
    println!(
        "{from}command=\"{} serve --stdio --peer {}\",restrict {public}",
        binary.display(),
        globals.host
    );
    println!("# Private key: {} (mode 0600)", key.display());
    println!("# Dedicated keys are optional; use_user_ssh_config routes need no enroll.");
    Ok(0)
}

fn cmd_link(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let parts = positional(args);
    let (verb, route_name) = match parts.as_slice() {
        [v] => (v.as_str(), None),
        [v, r, ..] => (v.as_str(), Some(r.clone())),
        [] => return Err(err("usage", "link run|up|down|status [ROUTE]")),
    };
    let teams = globals.load_teams()?;
    let routes = globals.load_routes()?;
    match verb {
        "status" => {
            let dir = globals.links_dir();
            let mut any = false;
            for (name, def) in &routes.routes {
                if def.driver != "ssh" {
                    continue;
                }
                if route_name.as_deref().is_some_and(|r| r != name) {
                    continue;
                }
                any = true;
                let state_file = dir.join(format!("{name}.json"));
                let state_text = std::fs::read_to_string(&state_file)
                    .unwrap_or_else(|_| "{\"state\":\"unknown\"}".to_string());
                println!("{name}: {}", state_text.trim());
            }
            if !any {
                println!("no ssh routes");
            }
            Ok(0)
        }
        "up" | "run" => {
            let name = route_name.ok_or_else(|| err("usage", "link up needs a ROUTE"))?;
            let def = routes
                .routes
                .get(&name)
                .ok_or_else(|| err("unknown-route", format!("route {name:?} is not defined")))?;
            if def.driver != "ssh" {
                return Err(err("usage", format!("route {name:?} is not an ssh route")));
            }
            // Find any host that uses this route, for its binary path.
            let remote_binary = teams
                .hosts
                .values()
                .find(|h| h.routes.contains(&name))
                .and_then(|h| h.agent_teams.clone())
                .unwrap_or_else(|| "agent-teams".to_string());
            let route = crate::route::SshRoute::from_def(
                &name,
                &def.raw,
                &globals.host,
                &remote_binary,
                &globals.links_dir(),
            );
            link_run(globals, &route, verb == "up")
        }
        "down" => {
            let name = route_name.ok_or_else(|| err("usage", "link down needs a ROUTE"))?;
            let socket = crate::route::control_socket(&name);
            let _ = std::process::Command::new("ssh")
                .args([
                    "-O",
                    "exit",
                    "-o",
                    &format!("ControlPath={}", socket.display()),
                    "x",
                ])
                .status();
            write_link_state(globals, &name, "down", None, 0);
            println!("{name}: down");
            Ok(0)
        }
        other => Err(err("usage", format!("unknown link verb {other:?}"))),
    }
}

fn write_link_state(
    globals: &Globals,
    route: &str,
    state: &str,
    boot_id: Option<&str>,
    failures: u32,
) {
    let value = json!({
        "state": state,
        "since": format_rfc3339_ms(now_unix_ms()),
        "last_ok": if state == "up" { json!(format_rfc3339_ms(now_unix_ms())) } else { Value::Null },
        "peer_boot_id": boot_id,
        "consecutive_failures": failures,
    });
    let _ = write_json_atomic(&globals.links_dir().join(format!("{route}.json")), &value);
}

fn link_run(
    globals: &Globals,
    route: &crate::route::SshRoute,
    once: bool,
) -> Result<i32, DataError> {
    // Ping through the route's master: our own socket for dedicated-key
    // routes, the owner's live master for use_user_ssh_config routes (no
    // control_path there, by design).
    if route.link_mode == "none" {
        return Err(err(
            "usage",
            format!(
                "route {:?} has link mode none; set link.mode on-demand or persistent",
                route.name
            ),
        ));
    }
    if let Some(parent) = route.control_path.as_ref().and_then(|c| c.parent()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut failures: u32 = 0;
    loop {
        match route.run_verb("ping", &json!({})) {
            AttemptOutcome::Answered(answer) => {
                failures = 0;
                write_link_state(globals, &route.name, "up", answer["boot_id"].as_str(), 0);
                if once {
                    println!(
                        "{}: up (peer boot {})",
                        route.name,
                        answer["boot_id"].as_str().unwrap_or("?")
                    );
                    return Ok(0);
                }
            }
            _ => {
                failures += 1;
                let state = if failures >= 3 { "down" } else { "degraded" };
                write_link_state(globals, &route.name, state, None, failures);
                if once {
                    println!("{}: {state}", route.name);
                    return Ok(1);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
}

/// Minimal base64 decoder for `--envelope-base64`.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    fn value(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let clean: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut out = Vec::new();
    for chunk in clean.chunks(4) {
        let mut acc: u32 = 0;
        let mut count = 0;
        for &b in chunk {
            if b == b'=' {
                break;
            }
            acc = (acc << 6) | value(b)? as u32;
            count += 1;
        }
        match count {
            2 => out.push((acc >> 4) as u8),
            3 => {
                out.push((acc >> 10) as u8);
                out.push((acc >> 2) as u8);
            }
            4 => {
                out.push((acc >> 16) as u8);
                out.push((acc >> 8) as u8);
                out.push(acc as u8);
            }
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synced_state_warning_flags_config_tree_only() {
        let config = Path::new("/home/u/.config/agent-teams");
        assert!(
            synced_state_warning(Path::new("/home/u/.config/agent-teams/state"), config).is_some()
        );
        assert!(synced_state_warning(Path::new("/home/u/.config/other"), config).is_some());
        assert!(
            synced_state_warning(Path::new("/home/u/.local/state/agent-teams"), config).is_none()
        );
    }
}
