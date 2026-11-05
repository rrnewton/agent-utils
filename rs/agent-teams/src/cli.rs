//! Command-line entry point: argument parsing, help, and dispatch.

use crate::deliver::deliverer_for;
use crate::envelope::{build_envelope, derived_key, message_id, team_tag};
use crate::jsonfile::{read_json, write_json_atomic, DataError};
use crate::ledger::{accept, fence, list_entries, read_entry, receipt, retract, AcceptContext};
use crate::model::{
    load_declaration, load_routes, load_teams, route_dirs, Declaration, RoutesFile, TeamsFile,
};
use crate::proof::{
    ancestry_has_agent, coordinator_liveness, identity_of, inside_unit, load_declarations,
    sender_standing, CoordinatorLiveness, SenderStanding,
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
                                       Verify declarations, contacts and routes,
                                       and flag unhealthy delivery targets
                                       (pinned, missing, archived, not live,
                                       child, wrong root) with their fixes.
  probe [--json]                       What this host's own tools say (for peers).
  import --coordinators-file FILE --team NAME [--json]
                                       Convert a per-host coordinators file; prints only.
  coordinators [--json]                Designated coordinators on this host, plus remote entries.
  join TEAM --registry R [--accepts-from S]...
                                       One-step team setup: declaration plus teams.json entry.

MESSAGING
  init TEAM (--registry R | --coordinator-self [--coordinator-pid PID])
      [--deliver agentctl-queue|hook-inbox|command] [--argv CMD...]
      [--accepts-from SENDER]... [--accepts-relay-from HUB]...
      [--relay ROUTE:CHANNEL:SENDER=TEAM[,SENDER=TEAM...]]...
  send TEAM[@HOST] (--file FILE | --text TEXT) [--key KEY | --again]
      [--topic SLUG] [--supersedes KEY]... [--ttl DUR | --expires TIME]
      [--kind info|request|hold|resume] [--private] [--from-service NAME]
      [--from-team TEAM] [--json]
                                       The sender team's declaration may allow-list
                                       destinations (may_send_to: team or team@host);
                                       refused destinations name the allowed front door.
  reply ID (--file FILE | --text TEXT) [--key KEY] [--json]
  retract ID [--json]
  status ID [--json]                   One message, from the outbox and receipts.
  outbox [--json]                      List the outbox.
  pump [--retry-refused] [--deferred] [--json]
                                       Retry deferred and ambiguous messages;
                                       --deferred retries only deferred ones.
  seen ID                              Has this host's ledger delivered ID?
  prune [TEAM]                         Drop terminal entries 30 days past not_after.
  resolve ID (--fence | --mark-delivered)   Human only, on the receiving host.
  hook [--team TEAM]                   Harness hook for the hook-inbox deliverer.

RECEIVING HOST (run by routes, not by hand)
  serve --stdio --peer HOST            Forced-command protocol endpoint.
  accept --protocol 1 [--team T] (--envelope - | --envelope-base64 B)
  receipt ID [--team T]                Pure read of the ledger.
  fence ID --team T --not-after TIME   Bar an ID before it arrives.
  drain-one --queue-dir D --registry R --agent A --id ID --ready-timeout S [--team T]
                                       Bounded delivery-on-arrival helper.

SSH AND LINKS
  ssh enroll --peer HOST [--from ADDR] Make a key; print the authorized_keys line.
  link run ROUTE | up/down ROUTE | status [ROUTE]
                                       Managed persistent ssh links.

CHAT RELAY (provider-neutral; Slack, Discord, or any bridge that reports
the poster's sender ID and can post into a channel)
  relay-in --route R --peer-sender ID [--channel C] [--thread T]
                                       Run by a chat bridge for one admitted
                                       relay post on stdin. Never typed as a
                                       prompt; envelopes go to the ledger,
                                       receipts update the outbox, posts for
                                       other teams are filtered silently.
  relay setup [--team T] [--provider slack|discord|generic]
      [--channel-name NAME] [--channel ID] [--route R] [--sender-id ID]
      [--peer ID=TEAM]... [--repo-url URL]
                                       Print owner steps and config snippets.
                                       Creates no channel; writes nothing.
  relay status [--json]                Relay routes, channels, candidates.
  relay announce --route R [--team T] [--sender-id ID] [--repo-url URL]
      [--print]                        Post (or print) this team's announce.
  relay teams [--repo ID | --repo-url URL] [--json]
                                       Teams learned in the channel.
  relay confirm SENDER [--team T] [--owner-approved] [--json]
                                       Confirm a candidate mapping, by a
                                       trusted-route check or owner approval.
                                       Unconfirmed claims are never admitted.
  relay trust-key TEAM@HOST [--replace] [--json]
                                       Trust only that team's signing key,
                                       read from its host's verified probe
                                       over a trusted route; maps no sender,
                                       so it works with a shared relay bot.
                                       Re-keying needs --replace. Releases
                                       quarantined posts claiming TEAM@HOST.
  relay keys [--json]                  List trusted signing keys.
  relay untrust-key TEAM@HOST          Remove a trusted signing key.
  relay reset-sig-v2 TEAM@HOST         Clear a trusted key's sticky v2
                                       record after that peer rolled back
                                       to a v1 build, so its sig-only
                                       envelopes are admitted again.

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
        "coordinators" => cmd_coordinators(&globals, cmd_args),
        "join" => cmd_join(&globals, cmd_args),
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
        "prune" => cmd_prune(&globals, cmd_args),
        "resolve" => cmd_resolve(&globals, cmd_args),
        "hook" => cmd_hook(&globals, cmd_args),
        "serve" => cmd_serve(&globals, cmd_args),
        "drain-one" => cmd_drain_one(&globals, cmd_args),
        "accept" => cmd_accept(&globals, cmd_args),
        "receipt" => cmd_receipt(&globals, cmd_args),
        "fence" => cmd_fence(&globals, cmd_args),
        "ssh" => cmd_ssh(&globals, cmd_args),
        "link" => cmd_link(&globals, cmd_args),
        "relay-in" => cmd_relay_in(&globals, cmd_args),
        "relay" => cmd_relay(&globals, cmd_args),
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
        "--channel",
        "--thread",
        "--channel-name",
        "--sender-id",
        "--repo",
        "--repo-url",
        "--provider",
        "--relay",
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
        for (svc, entry) in &host.services {
            println!(
                "  service {svc}: unit {}{}",
                entry.unit,
                entry
                    .team
                    .as_ref()
                    .map(|team| format!(" team {team}"))
                    .unwrap_or_default()
            );
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
            let effective = Some(route_names_for(globals, &teams, &routes, name, entry, None));
            println!(
                "  @{}{}: contacts [{}]{}",
                entry.host,
                entry
                    .coordinator
                    .as_ref()
                    .map(|c| format!(" coordinator={c} (informational)"))
                    .unwrap_or_default(),
                contacts.join(", "),
                effective
                    .map(|names| format!(" routes [{}]", names.join(", ")))
                    .unwrap_or_default()
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
        // A service that names its team (#861) must name a declared team.
        for (svc, entry) in &host.services {
            if let Some(team) = &entry.team {
                if !teams.teams.contains_key(team) {
                    problems.push(format!(
                        "host {name}: service {svc} names team {team:?}, which teams.json does not declare"
                    ));
                }
            }
        }
        let remote = name != &globals.host;
        if remote && host.agent_teams.is_none() && !host.routes.is_empty() {
            lines.push(json!({"kind": "warning", "detail": format!("host {name}: no agent_teams path; remote routes need one")}));
        }
    }
    // Teams and entries must name routes that exist (#770 per-team routes).
    for (name, team) in &teams.teams {
        for route_name in team.routes.iter().flatten() {
            if !routes.routes.contains_key(route_name) {
                problems.push(format!("team {name}: route {route_name:?} is not defined"));
            }
        }
        for entry in &team.entries {
            for route_name in entry.routes.iter().flatten() {
                if !routes.routes.contains_key(route_name) {
                    problems.push(format!(
                        "team {name}@{}: route {route_name:?} is not defined",
                        entry.host
                    ));
                }
            }
        }
        // An explicit relay in a team's or entry's list is an opt-in, but
        // the sending side still needs the team's declaration to name it;
        // warn when this host's declaration for the team does not.
        let explicit_relays: Vec<&String> = team
            .routes
            .iter()
            .flatten()
            .chain(team.entries.iter().flat_map(|e| e.routes.iter().flatten()))
            .filter(|route_name| {
                routes
                    .routes
                    .get(route_name.as_str())
                    .is_some_and(crate::route::is_relay_def)
            })
            .collect();
        if !explicit_relays.is_empty()
            && declarations.iter().any(|(t, _, _)| t == name)
            && !explicit_relays
                .iter()
                .all(|route_name| relay_enabled_for_team(globals, name, route_name))
        {
            lines.push(json!({"kind": "warning", "detail": format!(
                "team {name}: explicit routes name a relay ({}) but its declaration names no relay_channels for it; sends from {name} will not use it",
                explicit_relays.iter().map(|r| r.as_str()).collect::<Vec<_>>().join(", ")
            )}));
        }
    }
    // A relay route whose envelope limit exceeds its post cap can never
    // post the envelopes it admits: warn, do not refuse (#763).
    for def in routes.routes.values() {
        if crate::route::is_relay_def(def) {
            let max_post = def
                .raw
                .get("max_post_bytes")
                .and_then(Value::as_u64)
                .filter(|n| *n > 0)
                .unwrap_or(crate::relay::DEFAULT_MAX_POST_BYTES as u64);
            let max_envelope = def
                .raw
                .get("max_envelope_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(10_000);
            if max_envelope > max_post {
                lines.push(json!({"kind": "warning", "detail": format!(
                    "route {}: max_envelope_bytes {max_envelope} exceeds max_post_bytes {max_post}; envelopes over the post cap skip this route",
                    def.name
                )}));
            }
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
                        match coordinator_liveness(&declaration.designation, registry.as_deref()) {
                            CoordinatorLiveness::Live => {
                                lines.push(json!({"kind": "ok", "detail": format!("team {name}@{}: declaration live ({})", entry.host, declaration.deliverer_kind)}));
                            }
                            CoordinatorLiveness::NotLive => {
                                problems.push(format!(
                                    "team {name}@{}: designated coordinator is not live",
                                    entry.host
                                ));
                            }
                            CoordinatorLiveness::Unreachable(reason) => {
                                problems.push(format!(
                                    "team {name}@{}: designated coordinator liveness unverified (agentcloud orchestrator unreachable: {reason})",
                                    entry.host
                                ));
                            }
                        }
                    }
                    None => problems.push(format!(
                        "team {name}@{}: no declaration on this host; run agent-teams init {name}",
                        entry.host
                    )),
                }
            } else {
                match teams.hosts.get(&entry.host) {
                    Some(_host)
                        if !route_names_for(globals, &teams, &routes, name, entry, None)
                            .is_empty() =>
                    {
                        let names = route_names_for(globals, &teams, &routes, name, entry, None);
                        lines.push(json!({"kind": "ok", "detail": format!("team {name}@{}: routes [{}] (not probed)", entry.host, names.join(", "))}));
                    }
                    _ => problems.push(format!("team {name}@{}: no-route", entry.host)),
                }
            }
        }
    }
    // Delivery-target health (#721): a declaration pinned to a retired
    // coordinator, or resolving to a missing/archived/child agent, loses
    // messages silently - flag every declaration, named or not.
    for (team, _, declaration) in &declarations {
        if !targets.is_empty()
            && !targets
                .iter()
                .any(|t| t == team || t.starts_with(&format!("{team}@")))
        {
            continue;
        }
        // Errors lose messages today; warnings (a pin that goes stale
        // later, a tree shape) are problems only under --strict, like
        // every other warning here.
        let health = crate::health::evaluate(declaration);
        for flag in &health.flags {
            if strict || flag.is_error() {
                problems.push(format!("{} — fix: {}", flag.detail, flag.fix));
            }
            lines.push(json!({"kind": flag.kind, "severity": flag.severity, "detail": flag.detail, "fix": flag.fix}));
        }
    }
    // Team items land in the inbox.v2 lane; an agentctl predating lanes
    // would leave them untyped forever, so that is a hard problem here.
    let needs_lanes = declarations
        .iter()
        .any(|(_, _, declaration)| declaration.deliverer_kind == "agentctl-queue");
    if needs_lanes {
        match crate::proof::agentctl_supports_lanes() {
            Some(true) => lines.push(json!({"kind": "ok", "detail": format!("agentctl {} supports queue lanes (inbox.v2)", crate::proof::agentctl_binary().display())})),
            Some(false) => problems.push(format!(
                "agentctl {} predates queue lanes (inbox.v2); agentctl-queue items would never be typed; upgrade agentctl",
                crate::proof::agentctl_binary().display()
            )),
            None => lines.push(json!({"kind": "warning", "detail": format!("could not ask agentctl {} for its capabilities; lane support unverified", crate::proof::agentctl_binary().display())})),
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

/// `coordinators`: who coordinates what on this host (the per-host index
/// `agentctl` maintains at `coordinator set`/`clear`), plus the teams
/// file's entries on other hosts. Answers the owner's "which team-lead
/// coordinators are active here, in what directories, and how do I
/// message one" without scanning harnesses.
fn cmd_coordinators(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    // A missing/unreadable teams file lists nothing remotely; the local
    // index below is the primary source anyway.
    let teams = globals
        .load_teams()
        .unwrap_or_else(|_| crate::model::TeamsFile {
            raw: Value::Null,
            hosts: Default::default(),
            teams: Default::default(),
            hubs: Vec::new(),
            warnings: Vec::new(),
        });
    let routes_for_coords = globals.load_routes().unwrap_or_else(|_| RoutesFile {
        routes: Default::default(),
        warnings: Vec::new(),
    });
    let local: Vec<Value> = crate::hostindex::load()
        .iter()
        .map(|entry| {
            json!({
                "team": entry.team,
                "coordinator": entry.name,
                "registry": entry.registry.to_string_lossy(),
                "harness_cwd": entry.harness_cwd.to_string_lossy(),
                "repo_origins": entry.repo_origins,
                "where": globals.host,
                "live": crate::hostindex::live(entry),
            })
        })
        .collect();
    let mut remote: Vec<Value> = Vec::new();
    for (name, team) in &teams.teams {
        for entry in &team.entries {
            if entry.host != globals.host {
                remote.push(json!({
                    "team": name,
                    "coordinator": entry.coordinator,
                    "where": entry.host,
                    "routes": route_names_for(globals, &teams, &routes_for_coords, name, entry, None),
                }));
            }
        }
    }
    if globals.json || has_flag(args, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "host": globals.host, "local": local, "remote": remote,
            }))?
        );
        return Ok(0);
    }
    if local.is_empty() && remote.is_empty() {
        println!(
            "no coordinators known on {} (no host index entries, no remote teams)",
            globals.host
        );
        return Ok(0);
    }
    for row in &local {
        let state = if row["live"].as_bool() == Some(true) {
            "live"
        } else {
            "stale"
        };
        let team = row["team"].as_str().unwrap_or("(no team declared)");
        let repo = row["repo_origins"]
            .as_array()
            .filter(|list| !list.is_empty())
            .map(|list| format!(" repo {}", list[0].as_str().unwrap_or_default()))
            .unwrap_or_default();
        println!(
            "{team} @{}: coordinator {} ({state}) in {}{repo} - message: agent-teams send {team}",
            globals.host,
            row["coordinator"].as_str().unwrap_or("?"),
            row["harness_cwd"].as_str().unwrap_or("?"),
        );
    }
    for row in &remote {
        println!(
            "{} @{}: coordinator {} via routes [{}] - message: agent-teams send {}@{}",
            row["team"].as_str().unwrap_or("?"),
            row["where"].as_str().unwrap_or("?"),
            row["coordinator"].as_str().unwrap_or("(per declaration)"),
            row["routes"]
                .as_array()
                .map(|list| list
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default(),
            row["team"].as_str().unwrap_or("?"),
            row["where"].as_str().unwrap_or("?"),
        );
    }
    Ok(0)
}

/// `join TEAM --registry R ...`: one-step team setup on this host - the
/// declaration `init` writes, plus this host's entry in teams.json when
/// it is not there yet.
fn cmd_join(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let team = positional(args)
        .into_iter()
        .next()
        .ok_or_else(|| err("usage", "join needs a TEAM name"))?;
    let rc = cmd_init(globals, args)?;
    if rc != 0 {
        return Ok(rc);
    }
    // Add the entry to the teams file unless one for this host exists.
    let mut raw = globals
        .load_teams()
        .map(|teams| teams.raw)
        .unwrap_or_else(|_| json!({"schema": "agent-teams-teams/v1", "hosts": {}, "teams": {}}));
    let coordinator = crate::hostindex::live_coordinator_for_team(&team).map(|entry| entry.name);
    let teams_map = raw
        .get_mut("teams")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| err("config", "teams.json has no teams object"))?;
    let team_obj = teams_map.entry(team.clone()).or_insert_with(|| json!({}));
    let entries = team_obj
        .as_object_mut()
        .ok_or_else(|| err("config", format!("team {team} is not an object")))?
        .entry("entries")
        .or_insert_with(|| json!([]));
    let list = entries
        .as_array_mut()
        .ok_or_else(|| err("config", format!("team {team} entries is not a list")))?;
    if list
        .iter()
        .any(|entry| entry.get("host").and_then(Value::as_str) == Some(globals.host.as_str()))
    {
        println!("join: team {team} already has an entry on {}", globals.host);
        return Ok(0);
    }
    let mut entry = json!({"host": globals.host, "contacts": [{"kind": "host"}]});
    if let Some(name) = coordinator {
        entry["coordinator"] = json!(name);
    }
    // Entry ordering contract: local entry first (hosts skip their own
    // local route for the other entries; see model.rs).
    list.insert(0, entry);
    let path = globals.teams_file();
    crate::jsonfile::write_json_atomic(&path, &raw)?;
    println!(
        "join: added {team}@{} to {} (declaration written by init)",
        globals.host,
        path.display()
    );
    Ok(0)
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
    // Relay channels: --relay ROUTE:CHANNEL:SENDER=TEAM[,SENDER=TEAM...]
    // Several sender IDs may map to the same team.
    let mut relay_channels: Vec<Value> = Vec::new();
    for spec in flag_values(args, "--relay") {
        let mut parts = spec.splitn(3, ':');
        let (Some(route), Some(channel), Some(peers)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(err(
                "usage",
                format!("--relay {spec:?} must be ROUTE:CHANNEL:SENDER=TEAM[,SENDER=TEAM...]"),
            ));
        };
        let mut peer_map = serde_json::Map::new();
        for pair in peers.split(',') {
            let Some((sender, team)) = pair.split_once('=') else {
                return Err(err(
                    "usage",
                    format!("--relay peer {pair:?} must be SENDER=TEAM"),
                ));
            };
            peer_map.insert(sender.to_string(), json!(team));
        }
        relay_channels.push(json!({"route": route, "channel": channel, "peers": peer_map}));
    }
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
        "relay_channels": relay_channels,
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
        if declaration["relay_channels"]
            .as_array()
            .is_some_and(|a| a.is_empty())
        {
            if let Some(existing_channels) = existing.raw.get("relay_channels") {
                declaration["relay_channels"] = existing_channels.clone();
            }
        }
    }
    write_json_atomic(&path, &declaration)?;
    println!(
        "declared team {team} on {} ({})",
        globals.host,
        path.display()
    );
    if has_flag(args, "--coordinator-self") {
        println!(
            "warning: this declaration pins the current harness process; when that session is retired the pin goes stale and deliveries can be lost - prefer `agent-teams init {team} --registry <registry>`, which names only the registry and resolves its designated coordinator at send time (#721)"
        );
    }
    if declaration["deliverer"]
        .get("agent")
        .and_then(Value::as_str)
        .is_some()
    {
        println!(
            "warning: this declaration pins deliverer agent {:?}; a pinned name goes stale when that agent is retired - name the registry instead (#721)",
            declaration["deliverer"]["agent"].as_str().unwrap_or_default()
        );
    }
    if deliver == "hook-inbox" {
        println!("hook-inbox never wakes an idle agent. Add this harness hook:");
        println!("  command: agent-teams hook --team {team}   (UserPromptSubmit or Stop)");
    }
    Ok(0)
}

/// Which team a `--from-service` sender speaks for (#861): the service
/// entry's own `team` wins, then `--from-team`, then the legacy fallback of
/// the host's only declaration. With several declarations and no team named
/// either way, None: the caller refuses rather than guess.
fn service_team(
    entry: &crate::model::ServiceEntry,
    args: &[String],
    declaration_teams: &[String],
) -> Option<String> {
    entry
        .team
        .clone()
        .or_else(|| flag_value(args, "--from-team"))
        .or_else(|| {
            if declaration_teams.len() == 1 {
                Some(declaration_teams[0].clone())
            } else {
                None
            }
        })
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
        let entry = host.and_then(|h| h.services.get(&service)).ok_or_else(|| {
            err(
                "refused",
                format!(
                    "refused: sender-not-service (no service {service:?} registered for host {})",
                    globals.host
                ),
            )
        })?;
        let unit = &entry.unit;
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
        // The service sends as its own team (#861), never as whichever
        // declaration sorts first on this host.
        let declarations = load_declarations(&globals.state).unwrap_or_default();
        let declaration_teams: Vec<String> = declarations
            .iter()
            .map(|(name, _, _)| name.clone())
            .collect();
        let team = service_team(entry, args, &declaration_teams).ok_or_else(|| {
            err(
                "refused",
                format!(
                    "refused: sender-team-unknown (service {service:?} must name its team: set \
                     \"team\" in its services entry in teams.json or pass --from-team)"
                ),
            )
        })?;
        let own = declarations
            .into_iter()
            .find(|(name, _, _)| name == &team)
            .map(|(_, _, d)| d)
            .ok_or_else(|| {
                err(
                    "refused",
                    format!(
                        "refused: sender-team-undeclared (no declaration for team {team:?} on \
                         this host; declare it under the teams state dir or point the service \
                         at a declared team)"
                    ),
                )
            })?;
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

/// Whether the destination team is relay-enabled for one route: a local
/// declaration for that team names a `relay_channels` entry for the
/// route (#770/#782). The sender proves it from the declaration it can
/// read; a team it cannot prove relay-enabled gets no relay route and
/// no mirror — fail-closed, because relay routes expose the body.
fn relay_enabled_for_team(globals: &Globals, team: &str, route_name: &str) -> bool {
    crate::proof::load_declarations(&globals.state).is_ok_and(|declarations| {
        declarations.iter().any(|(name, _, declaration)| {
            name == team
                && declaration
                    .relay_channels
                    .iter()
                    .any(|channel| channel.route == route_name)
        })
    })
}

/// Whether a relay route may carry a message from one team to another:
/// BOTH teams must be relay-enabled for it. The destination check alone
/// is not enough — a sender with no relay declaration (and, for a
/// mirror, no signing key) would post bodies its own team never agreed
/// to expose, and the receiver would meet them as unsigned strangers.
fn relay_route_allowed(
    globals: &Globals,
    sender_decl_team: Option<&str>,
    team_name: &str,
    route_name: &str,
    explicit: bool,
) -> bool {
    // A team's or entry's own list is the destination's explicit opt-in;
    // otherwise the destination must declare the relay too.
    if !explicit && !relay_enabled_for_team(globals, team_name, route_name) {
        return false;
    }
    // The sender side has no explicit opt-in: its declaration must list
    // the route. Checks without a known sender (display, `check`) use
    // the destination side only.
    match sender_decl_team {
        Some(sender) => relay_enabled_for_team(globals, sender, route_name),
        None => true,
    }
}

/// The ordered route NAMES for one entry after the relay filter (#770):
/// the entry's own `routes` win, then its team's, then the host's
/// (older files: the host's, as today), with relay routes dropped unless
/// [`relay_route_allowed`] keeps them. `send` builds routes from this;
/// `check`, `show` and `coordinators` print it, so they agree.
fn route_names_for(
    globals: &Globals,
    teams: &TeamsFile,
    routes: &RoutesFile,
    team_name: &str,
    entry: &crate::model::Entry,
    sender_decl_team: Option<&str>,
) -> Vec<String> {
    let Some(host) = teams.hosts.get(&entry.host) else {
        return Vec::new();
    };
    let names = crate::model::effective_route_names(host, teams.teams.get(team_name), entry);
    let explicit: Vec<&String> = match (&entry.routes, teams.teams.get(team_name)) {
        (Some(list), _) => list.iter().collect(),
        (None, Some(team)) => team.routes.iter().flatten().collect(),
        (None, None) => Vec::new(),
    };
    names
        .into_iter()
        .filter(|name| {
            let is_relay = routes
                .routes
                .get(name.as_str())
                .is_some_and(crate::route::is_relay_def);
            !is_relay
                || relay_route_allowed(
                    globals,
                    sender_decl_team,
                    team_name,
                    name,
                    explicit.iter().any(|e| e.as_str() == name.as_str()),
                )
        })
        .collect()
}

/// The ordered candidate routes for one destination entry (#770).
fn routes_for_entry(
    globals: &Globals,
    teams: &TeamsFile,
    routes: &RoutesFile,
    team_name: &str,
    entry: &crate::model::Entry,
    sender_decl_team: Option<&str>,
) -> Result<Vec<Box<dyn Route>>, DataError> {
    let host_name = &entry.host;
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
    let names = route_names_for(globals, teams, routes, team_name, entry, sender_decl_team);
    let kept: Vec<&String> = names.iter().collect();
    build_routes(globals, routes, host_name, &remote_binary, &kept)
}

fn build_routes(
    globals: &Globals,
    routes: &RoutesFile,
    host_name: &str,
    remote_binary: &str,
    names: &[&String],
) -> Result<Vec<Box<dyn Route>>, DataError> {
    // The chat relay is always the fallback: it never precedes a direct
    // (ssh / node-exec / local) route, whatever order teams.json lists.
    let mut ordered: Vec<&String> = names.to_vec();
    ordered.sort_by_key(|name| {
        routes
            .routes
            .get(name.as_str())
            .is_some_and(crate::route::is_relay_def)
    });
    let mut out = Vec::new();
    for name in ordered {
        let def = routes
            .routes
            .get(name.as_str())
            .ok_or_else(|| err("unknown-route", format!("route {name:?} is not defined")))?;
        out.push(route_for(
            def,
            &globals.host,
            host_name,
            remote_binary,
            &globals.links_dir(),
        )?);
    }
    Ok(out)
}

/// Mirror routes for one message (#782 agent-teams-relay-mirror): every
/// relay route definition carrying `"mirror": true` that BOTH teams are
/// relay-enabled for and that the sender can sign for. Mirror routes are
/// found in `routes.d` whether or not any host lists them — mirroring is
/// a copy of a direct delivery, not a candidate route in the walk. The
/// signing key is required, not optional: an unsigned mirror copy is
/// indistinguishable from a stranger's post at the receiver.
fn mirror_routes_for(
    globals: &Globals,
    routes: &RoutesFile,
    team_name: &str,
    entry: &crate::model::Entry,
    sender_decl_team: &str,
    sender_key_team: &str,
) -> Vec<Box<dyn Route>> {
    if crate::relay::own_public_key(&globals.state, sender_key_team, &globals.host).is_none() {
        return Vec::new();
    }
    let remote_binary = "agent-teams".to_string();
    routes
        .routes
        .values()
        .filter(|def| crate::route::mirror_enabled(def))
        .filter(|def| {
            relay_route_allowed(globals, Some(sender_decl_team), team_name, &def.name, false)
        })
        .filter_map(|def| {
            route_for(
                def,
                &globals.host,
                &entry.host,
                &remote_binary,
                &globals.links_dir(),
            )
            .ok()
        })
        .collect()
}

/// The send allow list (#843): a team's declaration may name where its agents
/// may send, as `team` (any host) or `team@host` (only that host — the front
/// door). Same-team traffic is always allowed; an empty list allows everything
/// (today's behaviour). Returns the refusal text, naming the allowed front
/// door, when the destination is not allowed. The check runs on the sending
/// host, in `send` (and so in `reply`, which delegates to it); a relay/serve
/// hop does NOT re-check, because the envelope was already admitted by the
/// sender's host under the sender team's list — re-checking there would apply
/// the relay team's list to another team's traffic.
fn may_send_refusal(own: &Declaration, to_team: &str, to_host: &str) -> Option<String> {
    if to_team == own.team || own.may_send_to.is_empty() {
        return None;
    }
    let addressed = format!("{to_team}@{to_host}");
    let allowed = own
        .may_send_to
        .iter()
        .any(|entry| entry == to_team || entry == addressed.as_str());
    if allowed {
        return None;
    }
    let doors = own
        .may_send_to
        .iter()
        .map(|entry| {
            if entry.contains('@') {
                entry.clone()
            } else {
                format!("{entry}@*")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "refused: may-send-to (team {} may not send to {addressed}; allowed: {doors}; \
         same-team sends are always allowed)",
        own.team
    ))
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
    let from_team = sender
        .strip_prefix("service:")
        .map(|name| format!("svc.{name}"))
        .unwrap_or_else(|| sender.clone());
    let entry = match teams.pick_entry(&to_team, to_host_arg.as_deref(), &globals.host) {
        Ok(entry) => entry.clone(),
        Err(error) => {
            // Not in the teams file: the host coordinator index may know
            // the team on this host (another harness's coordinator). Same
            // user, same host: address it at its designated coordinator
            // through the local route.
            let local = if to_host_arg
                .as_deref()
                .is_none_or(|host| host == globals.host)
            {
                crate::hostindex::live_coordinator_for_team(&to_team)
            } else {
                None
            };
            match local {
                Some(found) => crate::model::Entry {
                    host: globals.host.clone(),
                    coordinator: Some(found.name.clone()),
                    contacts: vec![crate::model::Contact::Host],
                    routes: None,
                },
                None => return Err(error),
            }
        }
    };
    let to_host = entry.host.clone();
    if let Some(refusal) = may_send_refusal(&own, &to_team, &to_host) {
        return Err(err("refused", refusal));
    }
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
    // Replies get at least 24 hours by default (#770): the messages that
    // expired in the relay-fallback incident were mostly replies waiting
    // out a down route, and a relayed reply must also survive the
    // bridges' rescan window. An explicit --ttl/--expires always wins.
    // The receiver does not clamp to max_ttl_seconds — it REFUSES an
    // envelope whose not_after is beyond it — so where the destination's
    // declaration is readable here, cap the default at its max; where it
    // is not (a remote team), the destination must allow 24 h
    // (max_ttl_seconds >= 86400; the declaration default is 7 days).
    let default_ttl = if kind == "reply" {
        let wanted = default_ttl.max(86_400);
        let receiver_max = load_declarations(&globals.state)
            .ok()
            .and_then(|ds| ds.into_iter().find(|(t, _, _)| *t == to_team))
            .map(|(_, _, d)| d.max_ttl_seconds)
            .filter(|max| *max > 0);
        match receiver_max {
            Some(max) => wanted.min(max),
            None => wanted,
        }
    } else {
        default_ttl
    };
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
    let mut envelope = build_envelope(
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
    // Signed identity is the default once this team@host has a key
    // (created by `relay keygen` or `relay announce`): envelopes then
    // carry its signature, and the seed never leaves this host's state.
    // A team with no key yet sends unsigned under the legacy
    // one-bot-per-team mapping (the documented weaker mode).
    if crate::relay::own_public_key(&globals.state, &from_team, &globals.host).is_some() {
        let _ = crate::relay::sign_envelope(&globals.state, &mut envelope);
    }
    let proof = json!({
        "sender": sender, "proof": proof_kind, "host": globals.host,
        "pid": std::process::id(), "at": format_rfc3339_ms(now_ms),
    });
    let mut outbox_entry = write_outbox(&globals.state, &envelope, &addressed_to, &proof)?;
    // Local undeliverable pre-flight (#721): never walk a route to a local
    // declaration whose delivery target is missing or archived; say so
    // loudly and keep the message in the outbox for retry after the fix.
    if entry.host == globals.host {
        if let Ok(declarations) = load_declarations(&globals.state) {
            if let Some((_, _, declaration)) = declarations.iter().find(|(t, _, _)| t == &to_team) {
                let health = crate::health::evaluate(declaration);
                if let Some(flag) = health.undeliverable() {
                    // Stored as `deferred` with the reason beside it, the
                    // state every build's pump retries (#721).
                    outbox_entry["state"] = json!("deferred");
                    outbox_entry["undeliverable"] =
                        json!(format!("{} — fix: {}", flag.detail, flag.fix));
                    write_json_atomic(
                        &crate::sender::outbox_path(&globals.state, &id),
                        &outbox_entry,
                    )?;
                    if globals.json || has_flag(args, "--json") {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&json!({
                                "id": id, "key": key, "state": "deferred", "tag": team_tag(&envelope),
                                "undeliverable": outbox_entry["undeliverable"], "steps": [],
                            }))?
                        );
                    } else {
                        println!("{id}: undeliverable: {} — fix: {}", flag.detail, flag.fix);
                        println!("kept in the outbox as deferred; fix the target, then `agent-teams pump` (agentctl watch-agents pumps this host's outbox every 10 minutes)");
                    }
                    return Ok(75);
                }
            }
        }
    }
    let host_obj = teams
        .hosts
        .get(&to_host)
        .ok_or_else(|| err("unknown-host", format!("no host {to_host:?} in teams.json")))?
        .clone();
    let route_objs =
        routes_for_entry(globals, &teams, &routes, &to_team, &entry, Some(&from_team))?;
    let mirror_objs = mirror_routes_for(globals, &routes, &to_team, &entry, &from_team, &from_team);
    let result = crate::sender::walk(
        &globals.state,
        &mut outbox_entry,
        &entry,
        &host_obj,
        &route_objs,
        &mirror_objs,
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
        if let Some(reason) = outbox_entry["undeliverable"].as_str() {
            println!("undeliverable: {reason}");
            println!("kept in the outbox as deferred; it is retried by `agent-teams pump` (agentctl watch-agents pumps this host's outbox every 10 minutes)");
        }
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
            "{} {} -> {} key={}{}",
            row["id"].as_str().unwrap_or("?"),
            row["state"].as_str().unwrap_or("?"),
            row["addressed_to"].as_str().unwrap_or("?"),
            row["envelope"]["key"].as_str().unwrap_or("?"),
            row["undeliverable"]
                .as_str()
                .map(|reason| format!(" (undeliverable: {reason})"))
                .unwrap_or_default()
        );
    }
    Ok(0)
}

fn cmd_pump(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let retry_refused = has_flag(args, "--retry-refused");
    // Only messages that never reached a receiver (`agentctl watch-agents` pumps this way,
    // so an accepted message is not re-asked about every few minutes).
    let deferred_only = has_flag(args, "--deferred");
    // One pump at a time per state directory: two watchers sharing it, or a watcher and a
    // person, would otherwise walk the same entry at once and the last outbox write would win.
    // (`send` takes no lock, so a pump can still walk an entry a send is walking; the
    // receiver's ledger claim keeps that delivery single.)
    std::fs::create_dir_all(&globals.state)?;
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(globals.state.join("pump.lock"))?;
    if fs2::FileExt::try_lock_exclusive(&lock_file).is_err() {
        println!("pump: another pump is running on this state directory; nothing done");
        return Ok(0);
    }
    // Stranger alarms repeat on their timer whenever pump runs; under the
    // lock, so two pumps never post the same alarm or race the quarantine.
    let _ = crate::relay::raise_due_alarms(&globals.state, now_unix_ms());
    let _ = crate::relay::drain_quarantine(&globals.state, &globals.host);
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
            // In-custody (posted to the relay, awaiting its thread receipt)
            // gets a deadline: 15 minutes after its last attempt pump
            // re-walks it. Where a receipt-capable route exists the walk
            // asks for the receipt; on a relay-only route it surfaces as
            // ambiguous (loud, exit 76) instead of sitting silent forever.
            let last_attempt_ms = entry["attempts"]
                .as_array()
                .and_then(|list| list.last())
                .and_then(|attempt| attempt["at"].as_str())
                .and_then(crate::timeutil::parse_rfc3339_ms)
                .or_else(|| {
                    entry["created_at"]
                        .as_str()
                        .and_then(crate::timeutil::parse_rfc3339_ms)
                });
            let stale_custody = state == "in-custody"
                && last_attempt_ms.is_some_and(|at| now_unix_ms() - at > 900_000);
            // `undeliverable` is only what the first #721 build stored;
            // current builds store `deferred` with the reason beside it.
            let retryable = if deferred_only {
                matches!(state.as_str(), "deferred" | "undeliverable")
            } else {
                matches!(
                    state.as_str(),
                    "deferred" | "ambiguous" | "accepted" | "claimed" | "undeliverable"
                ) || stale_custody
                    || (retry_refused && state == "refused")
            };
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
            let sender_decl = entry["envelope"]["from"]["team"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let route_objs = routes_for_entry(
                globals,
                &teams,
                &routes,
                &to_team,
                &team_entry,
                Some(&sender_decl),
            )?;
            let mirror_objs = mirror_routes_for(
                globals,
                &routes,
                &to_team,
                &team_entry,
                &sender_decl,
                &sender_decl,
            );
            let result = crate::sender::walk(
                &globals.state,
                &mut entry,
                &team_entry,
                &host_obj,
                &route_objs,
                &mirror_objs,
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

/// `prune`: remove terminal ledger/outbox entries 30 days past their own
/// `not_after` (design 9.2). Prints one line per store that lost entries.
fn cmd_prune(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let now_ms = crate::timeutil::now_unix_ms();
    let wanted = positional(args).into_iter().next();
    let mut total = 0_usize;
    let teams = match &wanted {
        Some(team) => vec![(
            team.clone(),
            crate::ledger::prune_team(&globals.state, team, now_ms)?,
        )],
        None => crate::ledger::prune_all(&globals.state, now_ms)?,
    };
    for (team, removed) in &teams {
        if *removed > 0 {
            println!("pruned {removed} ledger entries in team {team}");
            total += removed;
        }
    }
    if wanted.is_none() {
        let removed = crate::ledger::prune_outbox(&globals.state, now_ms)?;
        if removed > 0 {
            println!("pruned {removed} outbox entries");
            total += removed;
        }
    }
    println!("prune: {total} entries removed");
    Ok(0)
}

fn cmd_seen(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let id = positional(args)
        .into_iter()
        .next()
        .ok_or_else(|| err("usage", "seen needs an ID"))?;
    // Fold any finished typing into the ledger before answering.
    if let Some((team, _)) = find_ledger_entry_any_team(globals, &id) {
        let _ = crate::ledger::reconcile_queue_delivery(&globals.state, &team, &id);
    }
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
        crate::jsonfile::create_private_dir_all(&claimed_dir)?;
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
    let team = flag_value(args, "--team");
    crate::deliver::run_auto_drain(
        &state,
        &queue_dir,
        &registry,
        &agent,
        &id,
        ready_timeout,
        team.as_deref(),
    );
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
        via_chat_relay: false,
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

/// `relay-in`: run by a chat bridge for one admitted relay post on stdin.
/// Prints one JSON object; `receipt_text` is for the caller to post in the
/// post's thread unless `receipt_posted` is true.
fn cmd_relay_in(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let route =
        flag_value(args, "--route").ok_or_else(|| err("usage", "relay-in needs --route R"))?;
    let peer_sender = flag_value(args, "--peer-sender")
        .ok_or_else(|| err("usage", "relay-in needs --peer-sender ID"))?;
    let channel = flag_value(args, "--channel").unwrap_or_default();
    let thread = flag_value(args, "--thread").unwrap_or_default();
    let in_thread = flag_value(args, "--thread-reply").as_deref() == Some("true");
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| err("io", e.to_string()))?;
    // Any relay activity re-raises overdue stranger alarms.
    let _ = crate::relay::raise_due_alarms(&globals.state, now_unix_ms());
    // Loop-back guard for this bridge's own bot posts: LEGACY MODE ONLY.
    // In signed mode the bot ID proves nothing (one bot may post for
    // every team), so `relay_in` decides from the signature itself: a
    // post signed by a confirmed peer key is delivered, a post signed
    // by our own key is ignored there. This shortcut therefore applies
    // only to unsigned envelopes, and never to one addressed to a local
    // team@host: such a post goes through `relay_in`, where an
    // inadmissible drop is a loud quarantined stranger, never silent.
    if let Ok(routes) = globals.load_routes() {
        let own_bot = routes.routes.values().any(|def| {
            crate::route::is_relay_def(def)
                && def.raw.get("sender_id").and_then(Value::as_str) == Some(peer_sender.as_str())
        });
        if own_bot {
            if let Ok(crate::relay::RelayPost::Envelope(envelope)) = crate::relay::parse_post(&text)
            {
                let signed = envelope
                    .get("relay")
                    .and_then(|relay| relay.get("pubkey"))
                    .and_then(Value::as_str)
                    .is_some();
                let to_team = envelope
                    .get("to")
                    .and_then(|to| to.get("team"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let to_host = envelope
                    .get("to")
                    .and_then(|to| to.get("host"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let addressed_here = to_host == globals.host
                    && load_declarations(&globals.state).is_ok_and(|declarations| {
                        declarations.iter().any(|(team, _, _)| team == to_team)
                    });
                if !signed && !addressed_here {
                    println!(
                        "{}",
                        json!({"outcome": "ignored-own-bot", "detail": "unsigned envelope posted by this bridge's own bot and addressed elsewhere; loop-back in legacy one-bot-per-team mode", "receipt_text": null, "receipt_posted": false})
                    );
                    return Ok(0);
                }
            }
        }
    }
    let out = crate::relay::relay_in(
        &globals.state,
        &globals.host,
        &route,
        &peer_sender,
        &channel,
        &thread,
        in_thread,
        &text,
    )?;
    // One compact JSON line: bridges parse the outcome from stdout.
    println!(
        "{}",
        json!({
            "outcome": out.outcome, "detail": out.detail,
            "receipt_text": out.receipt_text, "receipt_posted": out.receipt_posted,
        })
    );
    Ok(0)
}

fn cmd_relay(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let verb = positional(args).into_iter().next();
    match verb.as_deref() {
        Some("setup") => cmd_relay_setup(globals, args),
        Some("status") => cmd_relay_status(globals, args),
        Some("announce") => cmd_relay_announce(globals, args),
        Some("teams") => cmd_relay_teams(globals, args),
        Some("confirm") => cmd_relay_confirm(globals, args),
        Some("trust-key") => cmd_relay_trust_key(globals, args),
        Some("keys") => cmd_relay_keys(globals, args),
        Some("untrust-key") => cmd_relay_untrust_key(globals, args),
        Some("reset-sig-v2") => cmd_relay_reset_sig_v2(globals, args),
        Some("ban") => cmd_relay_ban(globals, args),
        Some("unban") => cmd_relay_unban(globals, args),
        Some("alarms") => cmd_relay_alarms(globals, args),
        Some("keygen") => cmd_relay_keygen(globals, args),
        Some("quarantine") => cmd_relay_quarantine(globals, args),
        _ => Err(err(
            "usage",
            "relay subcommands: setup, status, announce, teams, confirm, trust-key, keys, untrust-key, reset-sig-v2, ban, unban, alarms, keygen, quarantine",
        )),
    }
}

fn cmd_relay_quarantine(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    // args still carries the leading "quarantine" verb from cmd_relay.
    let positional = positional(args);
    let verb = positional.get(1).map(String::as_str);
    let target = positional.get(2).map(String::as_str);
    match verb {
        Some("list") | None => {
            let posts = crate::relay::quarantined_posts(&globals.state);
            if globals.json {
                println!("{}", serde_json::to_string_pretty(&json!(posts))?);
                return Ok(0);
            }
            if posts.is_empty() {
                println!("quarantine is empty");
            }
            for post in &posts {
                println!(
                    "{} from {} on {} ({}): {}",
                    post["id"].as_str().unwrap_or("?"),
                    post["peer_sender"].as_str().unwrap_or("?"),
                    post["route"].as_str().unwrap_or("?"),
                    post["quarantined_at"].as_str().unwrap_or("?"),
                    post["reason"].as_str().unwrap_or("?"),
                );
            }
            Ok(0)
        }
        Some("retry") => {
            let results = crate::relay::retry_quarantine(&globals.state, &globals.host, target)?;
            if results.is_empty() {
                println!("no quarantined post matched");
            }
            for (id, outcome) in &results {
                println!("{id}: {outcome}");
            }
            Ok(0)
        }
        Some(verb @ ("drop" | "dismiss")) => {
            let Some(id) = target else {
                return Err(err(
                    "usage",
                    "relay quarantine dismiss needs a quarantine ID",
                ));
            };
            if crate::relay::drop_quarantined(&globals.state, id)? {
                println!(
                    "{} quarantined post {id} (not delivered)",
                    if verb == "drop" {
                        "dropped"
                    } else {
                        "dismissed"
                    }
                );
                Ok(0)
            } else {
                println!("no quarantined post with ID {id}");
                Ok(1)
            }
        }
        _ => Err(err(
            "usage",
            "relay quarantine subcommands: list, retry [<id>], dismiss <id> (alias: drop)",
        )),
    }
}

fn cmd_relay_keygen(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let team = flag_value(args, "--team").ok_or_else(|| {
        err(
            "usage",
            "relay keygen needs --team T (the fleet whose key to create)",
        )
    })?;
    let (_, public) = crate::relay::ensure_signing_key(&globals.state, &team, &globals.host)?;
    println!("{team}@{} public key: {public}", globals.host);
    println!(
        "fingerprint: {}",
        &crate::envelope::sha256_hex(public.as_bytes())[..16]
    );
    println!(
        "the private seed stays in this host's agent-teams state (mode 0600) and is never posted"
    );
    Ok(0)
}

fn relay_route_defs(globals: &Globals) -> Result<Vec<crate::model::RouteDef>, DataError> {
    let routes = globals.load_routes()?;
    Ok(routes
        .routes
        .values()
        .filter(|def| crate::route::is_relay_def(def))
        .cloned()
        .collect())
}

fn cmd_relay_setup(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let provider = flag_value(args, "--provider").unwrap_or_else(|| "generic".to_string());
    let team = flag_value(args, "--team").unwrap_or_else(|| "<your-fleet>".to_string());
    // One shared relay channel for all fleets is the default layout
    // (#agent-teams-relay): routing is by signed envelope metadata, not by
    // channel. A per-fleet channel (#relay-<fleet>) stays an option with
    // --channel-name relay-<fleet> plus fleet scoping in the declaration.
    let channel_name =
        flag_value(args, "--channel-name").unwrap_or_else(|| "agent-teams-relay".to_string());
    let channel = flag_value(args, "--channel").unwrap_or_else(|| "<channel-id>".to_string());
    let route = flag_value(args, "--route").unwrap_or_else(|| "relay-coord".to_string());
    let sender_id =
        flag_value(args, "--sender-id").unwrap_or_else(|| "<your-bot-user-id>".to_string());
    let mut peers = serde_json::Map::new();
    for pair in flag_values(args, "--peer") {
        if let Some((sender, peer_team)) = pair.split_once('=') {
            peers.insert(sender.to_string(), json!(peer_team));
        }
    }
    if peers.is_empty() {
        peers.insert("<peer-bot-user-id>".to_string(), json!("<peer-team>"));
    }
    println!("# agent-teams chat relay setup (provider: {provider})");
    println!("# Nothing was created or written. The owner does step 1 by hand.");
    println!();
    println!("## 1. Owner: create the channel (agents never create channels)");
    match provider.as_str() {
        "discord" => {
            println!(
                "- In Discord, create a text channel named #{channel_name} in the shared server."
            );
            println!("- Invite each team's bot account to the server and give every bot permission to view #{channel_name}, read its history, and send messages there. Each team may use a different bot account; a team with two vibing bots invites both.");
        }
        "slack" => {
            println!("- In Slack, create a channel named #{channel_name} and note its channel ID.");
            println!("- Invite each team's Slack app (bot user) to #{channel_name}. Each team should post as its own app/bot user.");
        }
        _ => {
            println!("- Create one clearly named channel (suggested: #{channel_name}) on your chat provider and note its channel/space ID.");
            println!("- Add every team's bot/app account to the channel with permission to read history and post. Each team posts as its own bot account; one team may use several bot accounts.");
        }
    }
    println!();
    println!("## 2. Each team: note every bot user ID visible in the channel");
    println!("Run `agent-teams relay status` after configuring, announce with `agent-teams relay announce --route {route}`, and confirm peers with `agent-teams relay confirm` (see step 4). Proposed peer mapping for this host:");
    println!("{}", serde_json::to_string_pretty(&json!(peers))?);
    println!();
    println!("## 3a. Bridge configuration (agentctl chat): add to bridge.json config");
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "relay_channels": [{
                "channel": channel,
                "peer_senders": peers,
                "command": ["agent-teams", "relay-in", "--route", route],
                "comment": "Relay posts are never typed as prompts; they go only to this command."
            }]
        }))?
    );
    println!("The bridge appends --peer-sender, --channel and --thread when it runs the command.");
    if provider == "slack" {
        println!("Slack only: also add the same peers to the Slack backend configuration so the provider admits bot posts in this channel:");
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "backend_configuration": {"data": {"relay_channels": [{"channel": channel, "peer_senders": peers.keys().collect::<Vec<_>>()}]}}
            }))?
        );
    }
    println!();
    println!("## 3b. This team's declaration: agent-teams init {team} ... --relay {route}:{channel}:<SENDER=TEAM,...>");
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "relay_channels": [{
                "route": route, "channel": channel, "fleet": team, "peers": peers,
                "receipt_argv": ["<your-chat-outbound-helper>", "post", "--channel", "{channel}", "--thread", "{thread}", "--text-file", "{text_file}"],
                "comment": "fleet scopes this channel to one fleet (posts naming another fleet are refused); omit fleet for a channel shared by several fleets. receipt_argv posts receipts in the post thread; omit it and relay-in prints the receipt for its caller."
            }]
        }))?
    );
    println!();
    println!("## 3c. Sender route (routes.d): a command route in relay mode");
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "agent-teams-routes/v1",
            "routes": { route.clone(): {
                "driver": "command", "mode": "relay", "channel": channel,
                "sender_id": sender_id,
                "post_argv": ["<your-chat-outbound-helper>", "post", "--channel", "{channel}", "--text-file", "{text_file}"],
                "list_argv": ["<your-chat-outbound-helper>", "list", "--channel", "{channel}", "--message-id", "{message_id}"],
                "max_envelope_bytes": 10000, "limits": {"per_hour": 6},
                "visible_to_owner": true,
                "comment": "Bodies are visible to every channel member; --private messages never take this route."
            }}
        }))?
    );
    println!("Add {route:?} to the target host's routes list in teams.json (last: chat is the fallback route).");
    println!();
    println!("## 4. Confirm peers before trusting them");
    println!("Announcements make candidates only. `agent-teams relay teams` lists them; `agent-teams relay confirm <sender-id>` verifies over a trusted route when possible, else asks for one-line owner approval (`--owner-approved`). Never admit an unconfirmed claim.");
    let _ = globals;
    Ok(0)
}

fn cmd_relay_status(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let defs = relay_route_defs(globals)?;
    let declarations = load_declarations(&globals.state).unwrap_or_default();
    let store = crate::relay::load_store(&globals.state);
    let mut channels: Vec<Value> = Vec::new();
    for (_, _, declaration) in &declarations {
        for entry in &declaration.relay_channels {
            channels.push(json!({
                "team": declaration.team, "route": entry.route, "channel": entry.channel,
                "peers": entry.peers, "receipt_argv": entry.receipt_argv,
            }));
        }
    }
    let value = json!({
        "host": globals.host,
        "relay_routes": defs.iter().map(|def| json!({
            "name": def.name, "channel": def.raw.get("channel"),
            "sender_id": def.raw.get("sender_id"),
        })).collect::<Vec<_>>(),
        "declaration_channels": channels,
        "candidates": store.get("candidates").cloned().unwrap_or(json!([])),
        "confirmed": store.get("confirmed").cloned().unwrap_or(json!([])),
        "notes": [
            "A provider that cannot deliver other bots' posts cannot carry a relay on its live stream; the Slack rescan (90s by default, 24h window) is the fallback path and bounds latency.",
            "Candidates are never admitted. Only declaration/bridge peer mappings and confirmed peers speak for a team.",
        ],
    });
    if globals.json || has_flag(args, "--json") {
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(0);
    }
    for alarm in crate::relay::alarms(&globals.state)
        .iter()
        .filter(|alarm| alarm.get("state").and_then(Value::as_str) == Some("open"))
    {
        println!(
            "STRANGER ALARM: sender {} claims {}@{} -> team {} ({}) - confirm: agent-teams relay confirm {} - ban: agent-teams relay ban {}",
            alarm["sender_id"].as_str().unwrap_or("?"),
            alarm["claimed_from"]["team"].as_str().unwrap_or("?"),
            alarm["claimed_from"]["host"].as_str().unwrap_or("?"),
            alarm["local_team"].as_str().unwrap_or("?"),
            alarm["reason"].as_str().unwrap_or("?"),
            alarm["sender_id"].as_str().unwrap_or("?"),
            alarm["sender_id"].as_str().unwrap_or("?"),
        );
    }
    println!(
        "relay on {}: {} route(s), {} declaration channel(s)",
        globals.host,
        defs.len(),
        value["declaration_channels"].as_array().map_or(0, Vec::len)
    );
    for route in value["relay_routes"].as_array().into_iter().flatten() {
        println!(
            "  route {} channel {} sender {}",
            route["name"], route["channel"], route["sender_id"]
        );
    }
    for row in crate::relay::known_teams(&globals.state, None) {
        println!(
            "  {} sender {} team {}@{} repo {}",
            if row["confirmed"].as_bool() == Some(true) {
                "confirmed"
            } else {
                "candidate"
            },
            row["sender_id"].as_str().unwrap_or("?"),
            row["team"].as_str().unwrap_or("?"),
            row["host"].as_str().unwrap_or("?"),
            row["repo_id"].as_str().unwrap_or("?"),
        );
    }
    println!("note: candidates are never admitted until confirmed (relay confirm).");
    Ok(0)
}

fn cmd_relay_announce(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let route_name = flag_value(args, "--route")
        .ok_or_else(|| err("usage", "relay announce needs --route R"))?;
    let routes = globals.load_routes()?;
    let def = routes
        .routes
        .get(&route_name)
        .filter(|def| crate::route::is_relay_def(def))
        .ok_or_else(|| {
            err(
                "unknown-route",
                format!("route {route_name:?} is not a relay route"),
            )
        })?;
    let declarations = load_declarations(&globals.state)?;
    let team = match flag_value(args, "--team") {
        Some(team) => team,
        None => declarations
            .first()
            .map(|(team, _, _)| team.clone())
            .ok_or_else(|| {
                err(
                    "usage",
                    "relay announce needs --team or a local declaration",
                )
            })?,
    };
    let sender_id = flag_value(args, "--sender-id")
        .or_else(|| {
            def.raw
                .get("sender_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| {
            err(
                "usage",
                "relay announce needs --sender-id or sender_id in the route",
            )
        })?;
    let repo_url = flag_value(args, "--repo-url").or_else(|| {
        crate::hostindex::load()
            .into_iter()
            .find(|entry| entry.team.as_deref() == Some(team.as_str()))
            .and_then(|entry| entry.repo_origins.into_iter().next())
    });
    let mut announce = crate::relay::build_announce(
        &team,
        &globals.host,
        &sender_id,
        repo_url.as_deref().unwrap_or(""),
    );
    // The announce carries this team@host's public key (never the seed).
    let (_, public) = crate::relay::ensure_signing_key(&globals.state, &team, &globals.host)?;
    announce["pubkey"] = json!(public);
    announce["integrity"] = json!(crate::relay::announce_integrity(&announce));
    let text = crate::relay::render_announce(&announce);
    if has_flag(args, "--print") {
        print!("{text}");
        return Ok(0);
    }
    let route = crate::route::relay_route_for(def, &globals.links_dir());
    route
        .post_text(&text)
        .map_err(|reason| err("relay-post", reason))?;
    println!(
        "announced {team}@{} as {sender_id} in channel {}",
        globals.host, route.channel
    );
    Ok(0)
}

fn cmd_relay_teams(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let repo_filter = match (flag_value(args, "--repo"), flag_value(args, "--repo-url")) {
        (Some(repo), _) => Some(repo),
        (None, Some(url)) => Some(crate::relay::repo_id(&url)),
        (None, None) => None,
    };
    let rows = crate::relay::known_teams(&globals.state, repo_filter.as_deref());
    if globals.json || has_flag(args, "--json") {
        println!("{}", serde_json::to_string_pretty(&json!(rows))?);
        return Ok(0);
    }
    if rows.is_empty() {
        println!("no relay teams known here (no announcements seen yet)");
    }
    for row in &rows {
        println!(
            "{}: {}@{} sender {} repo {}{}",
            if row["confirmed"].as_bool() == Some(true) {
                "confirmed"
            } else {
                "candidate"
            },
            row["team"].as_str().unwrap_or("?"),
            row["host"].as_str().unwrap_or("?"),
            row["sender_id"].as_str().unwrap_or("?"),
            row["repo_id"].as_str().unwrap_or("?"),
            row.get("source")
                .and_then(Value::as_str)
                .map(|s| format!(" via {s}"))
                .unwrap_or_default(),
        );
    }
    Ok(0)
}

fn cmd_relay_confirm(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let sender = positional(args)
        .into_iter()
        .nth(1)
        .ok_or_else(|| err("usage", "relay confirm needs a SENDER id"))?;
    let store = crate::relay::load_store(&globals.state);
    let candidate = store
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|list| {
            list.iter()
                .rev()
                .find(|c| c.get("sender_id").and_then(Value::as_str) == Some(sender.as_str()))
        })
        .cloned();
    let Some(candidate) = candidate else {
        return Err(err(
            "unknown-sender",
            format!("no announcement candidate for sender {sender}; nothing to confirm"),
        ));
    };
    let team = candidate["team"].as_str().unwrap_or_default().to_string();
    if let Some(asked) = flag_value(args, "--team") {
        if asked != team {
            return Err(err(
                "usage",
                format!("candidate {sender} claims team {team}, not {asked}; confirm the claim as announced, or have the bot re-announce"),
            ));
        }
    }
    let host = candidate["host"].as_str().unwrap_or_default().to_string();
    let repo = candidate["repo_id"].as_str().map(str::to_string);
    // First try an already-trusted (non-relay) route: ask that host's own
    // agent-teams probe whether its declaration maps this sender to the
    // claimed team. The claim is never trusted from the channel alone.
    let mut trusted = false;
    if let Ok(teams) = globals.load_teams() {
        if let Ok(entry) = teams.pick_entry(&team, Some(&host), &globals.host) {
            let entry = entry.clone();
            if let Ok(all_routes) = globals.load_routes() {
                if let Some(host_obj) = teams.hosts.get(&entry.host).cloned() {
                    let _ = host_obj;
                    if let Ok(route_objs) =
                        routes_for_entry(globals, &teams, &all_routes, &team, &entry, None)
                    {
                        for route in route_objs.iter() {
                            if route.is_relay() {
                                continue;
                            }
                            if let AttemptOutcome::Answered(probe) =
                                route.run_verb("probe", &json!({}))
                            {
                                // A key confirmed over a trusted route: the
                                // host's own probe reports its public key.
                                let claimed_key = candidate
                                    .get("pubkey")
                                    .and_then(Value::as_str)
                                    .unwrap_or("");
                                let key_match = !claimed_key.is_empty()
                                    && probe
                                        .get("teams")
                                        .and_then(Value::as_array)
                                        .into_iter()
                                        .flatten()
                                        .filter(|t| {
                                            t.get("team").and_then(Value::as_str)
                                                == Some(team.as_str())
                                        })
                                        .any(|t| {
                                            t.get("relay_pubkey").and_then(Value::as_str)
                                                == Some(claimed_key)
                                        });
                                if key_match {
                                    trusted = true;
                                    break;
                                }
                                let mapped = probe
                                    .get("teams")
                                    .and_then(Value::as_array)
                                    .into_iter()
                                    .flatten()
                                    .filter(|t| {
                                        t.get("team").and_then(Value::as_str) == Some(team.as_str())
                                    })
                                    .any(|t| {
                                        t.get("relay_peers").and_then(Value::as_object).is_some_and(
                                            |peers| {
                                                peers
                                                    .get(&sender)
                                                    .and_then(Value::as_str)
                                                    .is_some_and(|mapped| {
                                                        // Mappings name TEAM@HOST: the fleet
                                                        // and, when named, the host must both
                                                        // match the claimed team@host.
                                                        let (fleet, mapped_host) =
                                                            match mapped.split_once('@') {
                                                                Some((fleet, host)) => {
                                                                    (fleet, Some(host))
                                                                }
                                                                None => (mapped, None),
                                                            };
                                                        fleet == team
                                                            && mapped_host.is_none_or(|h| h == host)
                                                    })
                                            },
                                        )
                                    });
                                if mapped {
                                    trusted = true;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if trusted {
        crate::relay::confirm_peer(
            &globals.state,
            &sender,
            &team,
            &host,
            repo.as_deref(),
            "trusted-route",
            candidate.get("route").and_then(Value::as_str),
        )?;
        if let Some(pubkey) = candidate.get("pubkey").and_then(Value::as_str) {
            if !pubkey.is_empty() {
                crate::relay::trust_key(&globals.state, &team, &host, pubkey, "trusted-route")?;
            }
        }
        crate::relay::close_alarm(&globals.state, &sender)?;
        let delivered = crate::relay::drain_quarantine(&globals.state, &globals.host)?;
        println!(
            "confirmed {sender} -> {team}@{host} (verified over a trusted route); delivered {} quarantined post(s): {}{}",
            delivered.delivered.len(),
            delivered.delivered.join(", "),
            if delivered.dropped.is_empty() {
                String::new()
            } else {
                format!("; dropped: {}", delivered.dropped.iter().map(|(id, state)| format!("{id} ({state})")).collect::<Vec<_>>().join(", "))
            }
        );
        return Ok(0);
    }
    if has_flag(args, "--owner-approved") {
        crate::relay::confirm_peer(
            &globals.state,
            &sender,
            &team,
            &host,
            repo.as_deref(),
            "owner",
            candidate.get("route").and_then(Value::as_str),
        )?;
        if let Some(pubkey) = candidate.get("pubkey").and_then(Value::as_str) {
            if !pubkey.is_empty() {
                crate::relay::trust_key(&globals.state, &team, &host, pubkey, "owner")?;
            }
        }
        crate::relay::close_alarm(&globals.state, &sender)?;
        let delivered = crate::relay::drain_quarantine(&globals.state, &globals.host)?;
        println!(
            "confirmed {sender} -> {team}@{host} (owner approval); delivered {} quarantined post(s): {}{}",
            delivered.delivered.len(),
            delivered.delivered.join(", "),
            if delivered.dropped.is_empty() {
                String::new()
            } else {
                format!("; dropped: {}", delivered.dropped.iter().map(|(id, state)| format!("{id} ({state})")).collect::<Vec<_>>().join(", "))
            }
        );
        return Ok(0);
    }
    println!("candidate {sender} claims {team}@{host}, but no trusted route confirms it.");
    println!("Owner: if this mapping is right, approve it with one line:");
    println!("  agent-teams relay confirm {sender} --owner-approved");
    println!("Until then it is not admitted anywhere.");
    Ok(1)
}

/// `relay trust-key TEAM@HOST`: trust ONLY a team's signing key, learned
/// from that host's own `probe` over an already-trusted (non-relay)
/// route. Unlike `relay confirm` it records no sender mapping, so it
/// works when several teams share one relay bot (where `confirm` must
/// refuse to remap the bot). Signed admission consults the trusted key
/// and the declaration's peer list, never the sender mapping.
fn cmd_relay_trust_key(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let target = positional(args)
        .into_iter()
        .nth(1)
        .ok_or_else(|| err("usage", "relay trust-key needs TEAM@HOST"))?;
    let (team, host) = target
        .split_once('@')
        .filter(|(team, host)| !team.is_empty() && !host.is_empty())
        .ok_or_else(|| {
            err(
                "usage",
                format!("relay trust-key needs TEAM@HOST, not {target:?}"),
            )
        })?;
    let (team, host) = (team.to_string(), host.to_string());
    let teams = globals.load_teams()?;
    let entry = teams
        .pick_entry(&team, Some(&host), &globals.host)
        .map_err(|e| err("unknown-target", e.to_string()))?
        .clone();
    let routes = globals.load_routes()?;
    let route_objs = routes_for_entry(globals, &teams, &routes, &team, &entry, None)?;
    let mut probed_key: Option<String> = None;
    for route in route_objs.iter() {
        if route.is_relay() {
            continue;
        }
        // A route that cannot reach the target host (a local route to
        // another host) must not be probed: it would answer with THIS
        // host's probe and bind this host's key as the target's (#790).
        if route.skip_reason(0).is_some() {
            continue;
        }
        if let AttemptOutcome::Answered(probe) = route.run_verb("probe", &json!({})) {
            if probe.get("host").and_then(Value::as_str) != Some(host.as_str()) {
                continue;
            }
            probed_key = probe
                .get("teams")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|t| t.get("team").and_then(Value::as_str) == Some(team.as_str()))
                .find_map(|t| t.get("relay_pubkey").and_then(Value::as_str))
                .map(str::to_string);
            if probed_key.is_some() {
                break;
            }
        }
    }
    let Some(pubkey) = probed_key.filter(|key| !key.is_empty()) else {
        println!(
            "no trusted route reports a relay signing key for {team}@{host}; nothing trusted. \
             Have that host run `agent-teams relay keygen --team {team}` and announce, or check \
             that a direct route to {host} works."
        );
        return Ok(1);
    };
    // Re-keying is explicit: a different key already trusted refuses
    // without --replace, and the change is printed either way.
    let replace = has_flag(args, "--replace");
    if let Some(old) = crate::relay::trusted_key(&globals.state, &team, &host) {
        if old != pubkey && !replace {
            println!(
                "{team}@{host} already trusts a different key:\n  old: {old}\n  new: {pubkey}\n\
                 Nothing changed. Re-run with --replace to re-key."
            );
            return Ok(1);
        }
        if old != pubkey {
            println!("replacing the trusted key for {team}@{host}:\n  old: {old}\n  new: {pubkey}");
        }
    }
    crate::relay::trust_key(&globals.state, &team, &host, &pubkey, "trusted-route")?;
    // Release quarantined posts claiming this team@host through signed
    // admission now that its key is trusted, and close their alarms.
    let released = crate::relay::retry_quarantine(&globals.state, &globals.host, None)?;
    let released_ids: Vec<&str> = released
        .iter()
        .filter(|(_, outcome)| matches!(outcome.as_str(), "accepted" | "known"))
        .map(|(id, _)| id.as_str())
        .collect();
    crate::relay::close_alarms_claiming(&globals.state, &team, &host)?;
    if globals.json || has_flag(args, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"team": team, "host": host, "pubkey": pubkey, "source": "trusted-route", "released": released_ids})
            )?
        );
    } else {
        println!(
            "trusted the signing key for {team}@{host} (from its host's probe over a trusted \
             route): {pubkey}\nNo sender was mapped: sender-to-team confirmation is unchanged, \
             so a shared relay bot keeps its existing mapping."
        );
        if !released_ids.is_empty() {
            println!(
                "released {} quarantined post(s) claiming {team}@{host}: {}",
                released_ids.len(),
                released_ids.join(", ")
            );
        }
    }
    Ok(0)
}

fn cmd_relay_keys(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let keys = crate::relay::trusted_keys(&globals.state);
    if globals.json || has_flag(args, "--json") {
        println!("{}", serde_json::to_string_pretty(&json!({"keys": keys}))?);
        return Ok(0);
    }
    if keys.is_empty() {
        println!("no trusted relay signing keys");
    }
    for key in &keys {
        println!(
            "{}@{}: {} (source {}, confirmed {})",
            key["team"].as_str().unwrap_or("?"),
            key["host"].as_str().unwrap_or("?"),
            key["pubkey"].as_str().unwrap_or("?"),
            key["source"].as_str().unwrap_or("?"),
            key["confirmed_at"].as_str().unwrap_or("?"),
        );
    }
    Ok(0)
}

fn cmd_relay_untrust_key(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let target = positional(args)
        .into_iter()
        .nth(1)
        .ok_or_else(|| err("usage", "relay untrust-key needs TEAM@HOST"))?;
    let (team, host) = target
        .split_once('@')
        .filter(|(team, host)| !team.is_empty() && !host.is_empty())
        .ok_or_else(|| {
            err(
                "usage",
                format!("relay untrust-key needs TEAM@HOST, not {target:?}"),
            )
        })?;
    if crate::relay::untrust_key(&globals.state, team, host)? {
        println!("removed the trusted signing key for {team}@{host}");
        Ok(0)
    } else {
        println!("no trusted signing key recorded for {team}@{host}");
        Ok(1)
    }
}

fn cmd_relay_reset_sig_v2(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let target = positional(args)
        .into_iter()
        .nth(1)
        .ok_or_else(|| err("usage", "relay reset-sig-v2 needs TEAM@HOST"))?;
    let (team, host) = target
        .split_once('@')
        .filter(|(team, host)| !team.is_empty() && !host.is_empty())
        .ok_or_else(|| {
            err(
                "usage",
                format!("relay reset-sig-v2 needs TEAM@HOST, not {target:?}"),
            )
        })?;
    if crate::relay::reset_trusted_key_sig_v2(&globals.state, team, host)? {
        println!(
            "cleared the sticky envelope-v2 record for {team}@{host}: its sig-only envelopes are admitted again until it next verifies a v2 signature"
        );
        Ok(0)
    } else {
        println!("no sticky envelope-v2 record for {team}@{host}");
        Ok(1)
    }
}

fn cmd_relay_ban(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let sender = positional(args)
        .into_iter()
        .nth(1)
        .ok_or_else(|| err("usage", "relay ban needs a SENDER id"))?;
    crate::relay::ban_sender(&globals.state, &sender)?;
    println!("banned {sender} in the relay: its alarm stops and its posts are dropped");
    Ok(0)
}

fn cmd_relay_unban(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let sender = positional(args)
        .into_iter()
        .nth(1)
        .ok_or_else(|| err("usage", "relay unban needs a SENDER id"))?;
    crate::relay::unban_sender(&globals.state, &sender)?;
    println!("unbanned {sender} in the relay: it is a stranger again until confirmed");
    Ok(0)
}

fn cmd_relay_alarms(globals: &Globals, args: &[String]) -> Result<i32, DataError> {
    let raised = crate::relay::raise_due_alarms(&globals.state, now_unix_ms())?;
    let all = crate::relay::alarms(&globals.state);
    let open: Vec<&Value> = all
        .iter()
        .filter(|alarm| alarm.get("state").and_then(Value::as_str) == Some("open"))
        .collect();
    if globals.json || has_flag(args, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"raised_now": raised, "alarms": all}))?
        );
        return Ok(0);
    }
    if open.is_empty() {
        println!("no open stranger alarms");
    }
    for alarm in &open {
        if alarm.get("kind").and_then(Value::as_str) == Some("integrity") {
            println!(
                "RELAY ALARM (integrity): confirmed sender {} -> team {} ({}; raised {}x, last {}) — dismiss: agent-teams relay quarantine dismiss {}",
                alarm["sender_id"].as_str().unwrap_or("?"),
                alarm["local_team"].as_str().unwrap_or("?"),
                alarm["reason"].as_str().unwrap_or("?"),
                alarm["raises"].as_u64().unwrap_or(0),
                alarm["last_raised_at"].as_str().unwrap_or("never"),
                alarm["quarantine_id"].as_str().unwrap_or("?"),
            );
            continue;
        }
        if alarm.get("kind").and_then(Value::as_str) == Some("unparseable") {
            println!(
                "RELAY ALARM (unparseable): confirmed sender {} -> team {} ({}; raised {}x, last {}) — dismiss: agent-teams relay quarantine dismiss {}",
                alarm["sender_id"].as_str().unwrap_or("?"),
                alarm["local_team"].as_str().unwrap_or("?"),
                alarm["reason"].as_str().unwrap_or("?"),
                alarm["raises"].as_u64().unwrap_or(0),
                alarm["last_raised_at"].as_str().unwrap_or("never"),
                alarm["quarantine_id"].as_str().unwrap_or("?"),
            );
            continue;
        }
        println!(
            "STRANGER ALARM: sender {} claims {}@{} -> team {} ({}; raised {}x, last {})",
            alarm["sender_id"].as_str().unwrap_or("?"),
            alarm["claimed_from"]["team"].as_str().unwrap_or("?"),
            alarm["claimed_from"]["host"].as_str().unwrap_or("?"),
            alarm["local_team"].as_str().unwrap_or("?"),
            alarm["reason"].as_str().unwrap_or("?"),
            alarm["raises"].as_u64().unwrap_or(0),
            alarm["last_raised_at"].as_str().unwrap_or("never"),
        );
    }
    println!(
        "quarantined posts: {}",
        crate::relay::quarantined_posts(&globals.state).len()
    );
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
    fn service_team_entry_wins_over_flag_and_first_declaration() {
        let entry = crate::model::ServiceEntry {
            unit: "nightly.service".to_string(),
            team: Some("team-b".to_string()),
        };
        let declarations = vec!["team-a".to_string(), "team-b".to_string()];
        // Several declarations: the entry's team, not the first sorted.
        assert_eq!(
            service_team(&entry, &[], &declarations).as_deref(),
            Some("team-b")
        );
        // The entry wins over --from-team too.
        let args = vec!["--from-team".to_string(), "team-a".to_string()];
        assert_eq!(
            service_team(&entry, &args, &declarations).as_deref(),
            Some("team-b")
        );
    }

    #[test]
    fn service_team_flag_then_single_declaration_legacy() {
        let entry = crate::model::ServiceEntry {
            unit: "nightly.service".to_string(),
            team: None,
        };
        let declarations = vec!["team-a".to_string(), "team-b".to_string()];
        let args = vec!["--from-team".to_string(), "team-b".to_string()];
        assert_eq!(
            service_team(&entry, &args, &declarations).as_deref(),
            Some("team-b")
        );
        // No team named anywhere and several declarations: refuse to guess.
        assert_eq!(service_team(&entry, &[], &declarations), None);
        // The single-declaration legacy case is unchanged.
        assert_eq!(
            service_team(&entry, &[], &["only-team".to_string()]).as_deref(),
            Some("only-team")
        );
    }

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

#[cfg(test)]
mod relay_gate_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn globals_with_declarations(teams: &[(&str, bool)]) -> (Globals, PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "agent-teams-relay-gate-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        for (team, relay) in teams {
            let sub = dir.join("teams.d").join(team);
            std::fs::create_dir_all(&sub).expect("mkdir");
            let channels = if *relay {
                json!([{"route": "relay-coord", "channel": "C1", "peers": {}}])
            } else {
                json!([])
            };
            std::fs::write(
                sub.join("declaration.json"),
                serde_json::to_string(&json!({
                    "schema": "agent-teams-declaration/v1", "team": team, "deliverer": {"kind": "hook-inbox"},
                    "relay_channels": channels,
                }))
                .expect("json"),
            )
            .expect("write");
        }
        (
            Globals {
                teams_path: None,
                routes_extra: Vec::new(),
                state: dir.clone(),
                config: dir.clone(),
                host: "host-a".to_string(),
                json: false,
            },
            dir,
        )
    }

    #[test]
    fn relay_needs_both_teams_and_services_have_no_declaration() {
        let (globals, dir) = globals_with_declarations(&[("tools", true), ("sim", true)]);
        // Both relay-enabled: allowed.
        assert!(relay_route_allowed(
            &globals,
            Some("sim"),
            "tools",
            "relay-coord",
            false
        ));
        // A service sender (svc.<name>) has no declaration: denied, as in pump.
        assert!(!relay_route_allowed(
            &globals,
            Some("svc.tick-hub"),
            "tools",
            "relay-coord",
            false
        ));
        // Destination explicit opt-in still requires the sending side.
        assert!(!relay_route_allowed(
            &globals,
            Some("svc.tick-hub"),
            "tools",
            "relay-coord",
            true
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let (globals, dir) = globals_with_declarations(&[("tools", true), ("sim", false)]);
        assert!(!relay_route_allowed(
            &globals,
            Some("sim"),
            "tools",
            "relay-coord",
            false
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
