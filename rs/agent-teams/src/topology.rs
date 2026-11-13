//! Topology fragments (`topology.d/<team>.json`, #1418): lead/subordinate
//! declarations, watcher defaults, and the check-in/probe/kick decisions.
//! Routes are resolved by agent-teams per host pair; fragments carry no
//! route names except an optional per-pair override. `team_key` is only a
//! per-host state-directory name on the target host; incident memory here
//! is keyed by team-instance id (`TEAM@HOST`), never by `team_key`.

use crate::jsonfile::{DataError, DataResult};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Fragment schema string.
pub const SCHEMA: &str = "agent-utils-topology/v1";
/// Default silence before a check-in (6 h).
pub const DEFAULT_CHECKIN_AFTER_SILENCE_SECONDS: i64 = 21_600;
/// Default silence before a peer probe (6 h).
pub const DEFAULT_PROBE_AFTER_SILENCE_SECONDS: i64 = 21_600;
/// Default age of an unanswered message before a peer probe (60 min).
pub const DEFAULT_PROBE_AFTER_UNANSWERED_SECONDS: i64 = 3_600;
/// Default escalation route file (#1417).
pub const DEFAULT_ESCALATION_CONFIG: &str = "~/.config/agent-utils/escalation.toml";

/// Optional watcher defaults as written in one fragment or the base file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Defaults {
    /// Escalation route file path, if set.
    pub escalation_config: Option<String>,
    /// Silence before a check-in, if set.
    pub checkin_after_silence_seconds: Option<i64>,
    /// Silence before a peer probe, if set.
    pub probe_after_silence_seconds: Option<i64>,
    /// Unanswered age before a peer probe, if set.
    pub probe_after_unanswered_seconds: Option<i64>,
}

/// Defaults with every value resolved for one watching team.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedDefaults {
    /// Escalation route file path.
    pub escalation_config: String,
    /// Silence before a check-in.
    pub checkin_after_silence_seconds: i64,
    /// Silence before a peer probe.
    pub probe_after_silence_seconds: i64,
    /// Unanswered age before a peer probe.
    pub probe_after_unanswered_seconds: i64,
}

/// One team instance (`TEAM@HOST`) as declared in its team's fragment.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    /// Team-instance id, `TEAM@HOST`.
    pub id: String,
    /// Registry path on the instance's own host.
    pub registry: String,
    /// Lead team-instance id, or none.
    pub lead: Option<String>,
    /// Team-instance ids this instance leads.
    pub subordinates: Vec<String>,
    /// `host` or `agentcloud`.
    pub coordinator_kind: String,
}

/// Optional per-pair route override for one operation set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteOverride {
    /// Override route for probes, if set.
    pub probe: Option<String>,
    /// Override route for kicks, if set.
    pub kick: Option<String>,
    /// Override route for check-ins, if set.
    pub checkin: Option<String>,
}

/// One parsed `topology.d/<team>.json` fragment.
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    /// Owning team; matches the file name.
    pub team: String,
    /// Defaults as written (all optional).
    pub defaults: Defaults,
    /// Per-pair overrides keyed by `<from-host>-><to-host>`.
    pub routes_override: BTreeMap<String, RouteOverride>,
    /// Declared instances of this team.
    pub instances: Vec<Instance>,
}

/// Merged topology: every declared instance plus its source fragments.
#[derive(Debug, Clone, Default)]
pub struct Topology {
    /// Instances by team-instance id.
    pub instances: BTreeMap<String, Instance>,
    /// Fragments by owning team, for watcher defaults.
    pub fragments: BTreeMap<String, Fragment>,
    /// Optional base-file defaults.
    pub base: Option<Defaults>,
}

fn err(message: impl Into<String>) -> DataError {
    DataError::new("topology", message)
}

fn req_str<'a>(obj: &'a Map<String, Value>, key: &str, ctx: &str) -> DataResult<&'a str> {
    obj.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| err(format!("{ctx}: missing string key {key}")))
}

fn opt_i64(obj: &Map<String, Value>, key: &str, ctx: &str) -> DataResult<Option<i64>> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .map(Some)
            .ok_or_else(|| err(format!("{ctx}: key {key} must be an integer"))),
    }
}

fn parse_defaults(obj: &Map<String, Value>, ctx: &str) -> DataResult<Defaults> {
    Ok(Defaults {
        escalation_config: obj
            .get("escalation_config")
            .and_then(Value::as_str)
            .map(str::to_string),
        checkin_after_silence_seconds: opt_i64(obj, "checkin_after_silence_seconds", ctx)?,
        probe_after_silence_seconds: opt_i64(obj, "probe_after_silence_seconds", ctx)?,
        probe_after_unanswered_seconds: opt_i64(obj, "probe_after_unanswered_seconds", ctx)?,
    })
}

/// Parse one fragment. `file_stem` is the file name without `.json`; the
/// fragment's `team` must match it.
pub fn parse_fragment(text: &str, file_stem: &str) -> DataResult<Fragment> {
    let value: Value = serde_json::from_str(text)?;
    let obj = value
        .as_object()
        .ok_or_else(|| err("fragment must be a JSON object"))?;
    if req_str(obj, "schema", "fragment")? != SCHEMA {
        return Err(err(format!("fragment schema must be {SCHEMA}")));
    }
    let team = req_str(obj, "team", "fragment")?.to_string();
    if team != file_stem {
        return Err(err(format!(
            "fragment team {team} does not match file name {file_stem}"
        )));
    }
    let mut routes_override = BTreeMap::new();
    if let Some(overrides) = obj.get("routes_override") {
        let overrides = overrides
            .as_object()
            .ok_or_else(|| err("routes_override must be an object"))?;
        for (pair, ops) in overrides {
            let ops = ops
                .as_object()
                .ok_or_else(|| err(format!("routes_override {pair} must be an object")))?;
            routes_override.insert(
                pair.clone(),
                RouteOverride {
                    probe: ops.get("probe").and_then(Value::as_str).map(str::to_string),
                    kick: ops.get("kick").and_then(Value::as_str).map(str::to_string),
                    checkin: ops
                        .get("checkin")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
            );
        }
    }
    let mut instances = Vec::new();
    let raw = obj
        .get("instances")
        .and_then(Value::as_object)
        .ok_or_else(|| err("fragment must carry an instances object"))?;
    for (id, entry) in raw {
        let entry = entry
            .as_object()
            .ok_or_else(|| err(format!("instance {id} must be an object")))?;
        let id_team = id.split('@').next().unwrap_or("");
        if id_team != team {
            return Err(err(format!(
                "instance {id} belongs to team {id_team}, not {team}"
            )));
        }
        let subordinates = match entry.get("subordinates") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| err(format!("instance {id}: subordinate must be a string")))
                })
                .collect::<DataResult<Vec<String>>>()?,
            Some(_) => return Err(err(format!("instance {id}: subordinates must be a list"))),
        };
        instances.push(Instance {
            id: id.clone(),
            registry: req_str(entry, "registry", &format!("instance {id}"))?.to_string(),
            lead: entry
                .get("lead")
                .and_then(Value::as_str)
                .map(str::to_string),
            subordinates,
            coordinator_kind: entry
                .get("coordinator_kind")
                .and_then(Value::as_str)
                .unwrap_or("host")
                .to_string(),
        });
    }
    Ok(Fragment {
        team: team.clone(),
        defaults: parse_defaults(obj, "fragment")?,
        routes_override,
        instances,
    })
}

/// Read every `*.json` fragment in `dir`, in file-name order. Parse and
/// read failures are returned as problem strings naming the file, so a
/// check can report them alongside merge conflicts.
pub fn load_fragments(dir: &std::path::Path) -> (Vec<Fragment>, Vec<String>) {
    let mut fragments = Vec::new();
    let mut problems = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            return (
                fragments,
                vec![format!("topology dir {}: {e}", dir.display())],
            );
        }
    };
    let mut paths: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    for path in paths {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        match std::fs::read_to_string(&path) {
            Ok(text) => match parse_fragment(&text, &stem) {
                Ok(fragment) => fragments.push(fragment),
                Err(e) => problems.push(format!("{}: {e}", path.display())),
            },
            Err(e) => problems.push(format!("{}: {e}", path.display())),
        }
    }
    (fragments, problems)
}

/// Merge fragments (and an optional base's defaults) at read time.
/// Conflicting instance declarations are returned as conflict strings and
/// never silently merged; defaults are per-watcher policy and are never
/// compared across fragments (design section 2.3).
pub fn merge(fragments: Vec<Fragment>, base: Option<Defaults>) -> Result<Topology, Vec<String>> {
    let mut conflicts: Vec<String> = Vec::new();
    let mut topology = Topology {
        base,
        ..Topology::default()
    };
    for fragment in fragments {
        for instance in &fragment.instances {
            if topology.instances.contains_key(&instance.id) {
                conflicts.push(format!("instance {} declared twice", instance.id));
            } else {
                topology
                    .instances
                    .insert(instance.id.clone(), instance.clone());
            }
            if instance.lead.as_deref() == Some(instance.id.as_str()) {
                conflicts.push(format!("instance {} names itself as lead", instance.id));
            }
            for sub in &instance.subordinates {
                if sub == &instance.id {
                    conflicts.push(format!(
                        "instance {} names itself as subordinate",
                        instance.id
                    ));
                }
            }
        }
        topology.fragments.insert(fragment.team.clone(), fragment);
    }
    for instance in topology.instances.values() {
        if let Some(lead) = &instance.lead {
            if !topology.instances.contains_key(lead) {
                conflicts.push(format!(
                    "instance {} names undeclared lead {lead}",
                    instance.id
                ));
            }
        }
        for sub in &instance.subordinates {
            match topology.instances.get(sub) {
                None => conflicts.push(format!(
                    "instance {} names undeclared subordinate {sub}",
                    instance.id
                )),
                Some(sub_instance) => {
                    if sub_instance.lead.as_deref() != Some(instance.id.as_str()) {
                        conflicts.push(format!(
                            "instance {} lists {sub} as subordinate but {sub} does not name it as lead",
                            instance.id
                        ));
                    }
                }
            }
        }
    }
    if conflicts.is_empty() {
        Ok(topology)
    } else {
        Err(conflicts)
    }
}

impl Topology {
    /// Resolved defaults for one watching team: its own fragment, then
    /// the base, then the built-ins. Another team's fragment never applies.
    pub fn defaults_for(&self, team: &str) -> ResolvedDefaults {
        let own = self.fragments.get(team).map(|f| &f.defaults);
        let pick = |get: &dyn Fn(&Defaults) -> Option<i64>| -> Option<i64> {
            own.and_then(get)
                .or_else(|| self.base.as_ref().and_then(get))
        };
        ResolvedDefaults {
            escalation_config: own
                .and_then(|d| d.escalation_config.clone())
                .or_else(|| self.base.as_ref().and_then(|d| d.escalation_config.clone()))
                .unwrap_or_else(|| DEFAULT_ESCALATION_CONFIG.to_string()),
            checkin_after_silence_seconds: pick(&|d: &Defaults| d.checkin_after_silence_seconds)
                .unwrap_or(DEFAULT_CHECKIN_AFTER_SILENCE_SECONDS),
            probe_after_silence_seconds: pick(&|d: &Defaults| d.probe_after_silence_seconds)
                .unwrap_or(DEFAULT_PROBE_AFTER_SILENCE_SECONDS),
            probe_after_unanswered_seconds: pick(&|d: &Defaults| d.probe_after_unanswered_seconds)
                .unwrap_or(DEFAULT_PROBE_AFTER_UNANSWERED_SECONDS),
        }
    }

    /// Watched peers of one instance: its lead plus its subordinates.
    /// There is no `peers` key; this relation is the whole definition,
    /// and a subordinate always watches (and may kick) upward.
    pub fn watched_peers(&self, instance_id: &str) -> Vec<String> {
        let mut peers: Vec<String> = Vec::new();
        if let Some(instance) = self.instances.get(instance_id) {
            if let Some(lead) = &instance.lead {
                peers.push(lead.clone());
            }
            peers.extend(instance.subordinates.iter().cloned());
        }
        peers
    }
}

/// Validate `routes_override` route names against the routes declared for
/// each destination host. Returns one problem string per bad entry.
pub fn validate_routes(
    topology: &Topology,
    declared_routes: &dyn Fn(&str) -> BTreeSet<String>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for fragment in topology.fragments.values() {
        for (pair, ops) in &fragment.routes_override {
            let Some((_, to_host)) = pair.split_once("->") else {
                problems.push(format!(
                    "routes_override key {pair} must be <from-host>-><to-host>"
                ));
                continue;
            };
            let declared = declared_routes(to_host);
            for route in [&ops.probe, &ops.kick, &ops.checkin].into_iter().flatten() {
                if !declared.contains(route) {
                    problems.push(format!(
                        "routes_override {pair} names undeclared route {route} for host {to_host}"
                    ));
                }
            }
        }
    }
    problems
}

/// What a peer probe found, without asking an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerHealth {
    /// Driver state healthy.
    Healthy,
    /// Deliberately held (for example a turnover pin): not unhealthy.
    Held,
    /// Driver state unhealthy.
    Unhealthy,
    /// Peer lead unreachable.
    LeadUnreachable,
}

/// Whether a probe result justifies kicking the peer's driver. A kick
/// revives only; it never starts gates or touches slots or bridges.
pub fn should_kick(health: PeerHealth) -> bool {
    matches!(health, PeerHealth::Unhealthy | PeerHealth::LeadUnreachable)
}

/// Silence bookkeeping for one watched peer, kept per host, never synced.
#[derive(Debug, Clone, Default)]
pub struct Watch {
    /// Last time any agent-teams traffic arrived from the peer.
    pub last_seen_unix: Option<i64>,
    /// When the oldest unanswered message to the peer was sent.
    pub unanswered_since_unix: Option<i64>,
}

/// A due watcher action for one peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// Send the 6-hour check-in.
    Checkin,
    /// Read the peer's driver state and health.
    Probe,
}

/// Actions due at `now_unix` for one watched peer under the watcher's
/// resolved defaults.
pub fn due(now_unix: i64, watch: &Watch, defaults: &ResolvedDefaults) -> Vec<Due> {
    let mut actions = Vec::new();
    let silent_for = watch
        .last_seen_unix
        .map(|seen| now_unix - seen)
        .unwrap_or(i64::MAX);
    if silent_for >= defaults.checkin_after_silence_seconds {
        actions.push(Due::Checkin);
    }
    let unanswered_for = watch
        .unanswered_since_unix
        .map(|since| now_unix - since)
        .unwrap_or(i64::MIN);
    if silent_for >= defaults.probe_after_silence_seconds
        || unanswered_for >= defaults.probe_after_unanswered_seconds
    {
        actions.push(Due::Probe);
    }
    actions
}

/// Minimal view of the peer's frozen driver state
/// (`agentctl-team-driver/v1`), read remotely by a probe. The probe never
/// asks an agent; it reads this state (or the driver's health verdict).
#[derive(Debug, Clone, PartialEq)]
pub struct DriverSnapshot {
    /// Driver state name, for example `HEALTHY`.
    pub state: String,
    /// Frozen per-epoch incident id from the driver state.
    pub incident: String,
    /// The peer reports itself deliberately held (for example a turnover
    /// pin): held reads as held, never as unhealthy.
    pub held: bool,
}

/// Map a probed driver snapshot to peer health.
pub fn health_of(snapshot: &DriverSnapshot) -> PeerHealth {
    if snapshot.held {
        PeerHealth::Held
    } else if snapshot.state == "HEALTHY" {
        PeerHealth::Healthy
    } else {
        PeerHealth::Unhealthy
    }
}

/// I/O seams for one watcher tick, so fixture tests script every outcome
/// with fakes and no live registry, unit, route or binary is touched.
/// Implementations resolve routes through agent-teams per host pair and
/// run the peer's kick through its driver (`agentctl team kick`, #1414).
pub trait PeerIo {
    /// Send the peer a check-in; its lead answers in one line.
    fn send_checkin(&mut self, peer: &str) -> Result<(), String>;
    /// Read the peer's driver state remotely. `Err` means the peer's
    /// lead and driver could not be reached at all.
    fn probe_driver(&mut self, peer: &str) -> Result<DriverSnapshot, String>;
    /// Kick the peer's driver once. Idempotent at the driver.
    fn kick(&mut self, peer: &str) -> Result<(), String>;
    /// Notify the kicker's own lead and the owner, once per incident.
    fn notify(&mut self, peer: &str, incident: &str) -> Result<(), String>;
}

/// What one watcher tick did for one peer, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A check-in was sent.
    CheckinSent,
    /// The peer's driver state was read.
    Probed,
    /// The peer's driver was kicked for this incident.
    Kicked,
    /// Lead and owner were notified for this incident.
    Notified,
}

/// Run one watcher tick for one watched peer: check in after silence,
/// probe after silence or an unanswered message, kick an unhealthy or
/// unreachable peer exactly once per incident, and notify once per
/// incident. A kick revives only; the kicker does no slot or bridge
/// surgery and never starts gates. Returns the events performed.
pub fn tick(
    now_unix: i64,
    peer: &str,
    watch: &Watch,
    defaults: &ResolvedDefaults,
    io: &mut dyn PeerIo,
    ledger: &mut IncidentLedger,
) -> Vec<Event> {
    let mut events = Vec::new();
    let actions = due(now_unix, watch, defaults);
    if actions.contains(&Due::Checkin) && io.send_checkin(peer).is_ok() {
        events.push(Event::CheckinSent);
    }
    if !actions.contains(&Due::Probe) {
        return events;
    }
    let incident_key = match io.probe_driver(peer) {
        Ok(snapshot) => {
            events.push(Event::Probed);
            if !should_kick(health_of(&snapshot)) {
                return events;
            }
            snapshot.incident.clone()
        }
        Err(_) => {
            // Unreachable: no snapshot, so the incident is the peer's
            // silence itself; keyed by peer, it still kicks only once
            // until the peer answers again and a new silence begins.
            String::new()
        }
    };
    let incident = if incident_key.is_empty() {
        format!("unreachable:{}", watch.last_seen_unix.unwrap_or(0))
    } else {
        incident_key
    };
    if ledger.claim_kick(peer, &incident) && io.kick(peer).is_ok() {
        events.push(Event::Kicked);
        if io.notify(peer, &incident).is_ok() {
            events.push(Event::Notified);
        }
    }
    events
}

/// Parse a peer's frozen driver state (`agentctl-team-driver/v1`, read
/// on the target host or fetched remotely) into a probe snapshot.
/// Unknown v1 fields are ignored, per the freeze.
pub fn parse_driver_state(text: &str) -> DataResult<DriverSnapshot> {
    let value: Value = serde_json::from_str(text)?;
    let obj = value
        .as_object()
        .ok_or_else(|| err("driver state must be a JSON object"))?;
    if obj.get("schema").and_then(Value::as_str) != Some("agentctl-team-driver/v1") {
        return Err(err("driver state schema must be agentctl-team-driver/v1"));
    }
    Ok(DriverSnapshot {
        state: obj
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        incident: obj
            .get("incident")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        held: obj.get("held").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// Kick argv for one peer registry (frozen `team kick` shape, #1414).
/// The route that carries this argv to the peer's host is resolved by
/// agent-teams per host pair; no route name appears here.
pub fn kick_argv(registry: &str, escalation_config: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "agentctl".to_string(),
        "--registry".to_string(),
        registry.to_string(),
        "team".to_string(),
        "kick".to_string(),
    ];
    if let Some(path) = escalation_config {
        argv.push("--escalation-config".to_string());
        argv.push(path.to_string());
    }
    argv
}

/// Check-in argv: one agent-teams message to the peer's coordinator.
pub fn checkin_argv(peer: &str) -> Vec<String> {
    vec![
        "agent-teams".to_string(),
        "send".to_string(),
        peer.to_string(),
        "--text".to_string(),
        "team check-in: healthy?".to_string(),
        "--kind".to_string(),
        "info".to_string(),
        "--topic".to_string(),
        "team-checkin".to_string(),
    ]
}

/// Runs one argv and returns stdout; the real implementation spawns the
/// process, fixture tests script it. No live anything is touched in tests.
pub trait CommandRunner {
    /// Run argv; `Err` carries one line of detail.
    fn run(&mut self, argv: &[String]) -> Result<String, String>;
}

/// The real runner: spawn argv, return stdout on exit 0. A non-zero exit
/// (for example kick's exit 1 refusal inside a non-initial PID namespace,
/// whose reason text is preserved) or a spawn failure is `Err` with the
/// first stderr line.
pub struct ProcessRunner;

impl CommandRunner for ProcessRunner {
    fn run(&mut self, argv: &[String]) -> Result<String, String> {
        let (program, rest) = argv.split_first().ok_or_else(|| "empty argv".to_string())?;
        let out = std::process::Command::new(program)
            .args(rest)
            .output()
            .map_err(|e| format!("{program}: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let first = stderr.lines().next().unwrap_or("").trim();
            Err(if first.is_empty() {
                format!("{program} exited {}", out.status)
            } else {
                first.to_string()
            })
        }
    }
}

/// One peer as the adapter needs it: where its registry lives, and the
/// command template (with `{registry}` substituted) that prints its
/// frozen driver state. The template is configuration, never hard-coded,
/// because the remote-exec path differs per host pair.
#[derive(Debug, Clone)]
pub struct PeerTarget {
    /// Registry path on the peer's host.
    pub registry: String,
    /// Argv template printing the peer's `state.json`; `{registry}` is
    /// replaced with `registry`.
    pub read_state_argv: Vec<String>,
}

/// The real `PeerIo`: check-in through agent-teams, probe by reading the
/// peer's frozen driver state, kick through the peer's driver, notify as
/// an agent-teams message to the kicker's own lead (the caller supplies
/// that argv, since the lead's address is the kicker's own config).
pub struct CommandPeerIo<'a> {
    /// Peers by team-instance id.
    pub targets: &'a BTreeMap<String, PeerTarget>,
    /// Escalation route file passed to kicks, if configured.
    pub escalation_config: Option<&'a str>,
    /// Builds the notify argv for (peer, incident).
    pub notify_argv: &'a dyn Fn(&str, &str) -> Vec<String>,
    /// Process runner (real or scripted).
    pub runner: &'a mut dyn CommandRunner,
}

impl PeerIo for CommandPeerIo<'_> {
    fn send_checkin(&mut self, peer: &str) -> Result<(), String> {
        self.runner.run(&checkin_argv(peer)).map(|_| ())
    }

    fn probe_driver(&mut self, peer: &str) -> Result<DriverSnapshot, String> {
        let target = self
            .targets
            .get(peer)
            .ok_or_else(|| format!("no target for peer {peer}"))?;
        let argv: Vec<String> = target
            .read_state_argv
            .iter()
            .map(|a| a.replace("{registry}", &target.registry))
            .collect();
        let text = self.runner.run(&argv)?;
        parse_driver_state(&text).map_err(|e| e.message)
    }

    fn kick(&mut self, peer: &str) -> Result<(), String> {
        let target = self
            .targets
            .get(peer)
            .ok_or_else(|| format!("no target for peer {peer}"))?;
        self.runner
            .run(&kick_argv(&target.registry, self.escalation_config))
            .map(|_| ())
    }

    fn notify(&mut self, peer: &str, incident: &str) -> Result<(), String> {
        self.runner
            .run(&(self.notify_argv)(peer, incident))
            .map(|_| ())
    }
}

/// Kicked-incident memory, keyed by team-instance id (`TEAM@HOST`), never
/// by `team_key`. Shared by every prober on one host through one state
/// file, so several probers still produce exactly one kick per incident.
#[derive(Debug, Clone, Default)]
pub struct IncidentLedger {
    /// Incident ids already kicked, per team-instance id.
    pub kicked: BTreeMap<String, BTreeSet<String>>,
}

impl IncidentLedger {
    /// Claim the kick for one incident: true only for the first claimer.
    pub fn claim_kick(&mut self, instance_id: &str, incident: &str) -> bool {
        self.kicked
            .entry(instance_id.to_string())
            .or_default()
            .insert(incident.to_string())
    }

    /// Serialize for the per-host state file.
    pub fn to_json(&self) -> Value {
        json!({ "kicked": self.kicked })
    }

    /// Load from the per-host state file.
    pub fn from_json(value: &Value) -> DataResult<Self> {
        let mut kicked: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        if let Some(map) = value.get("kicked").and_then(Value::as_object) {
            for (instance, incidents) in map {
                let incidents = incidents
                    .as_array()
                    .ok_or_else(|| err(format!("ledger for {instance} must be a list")))?;
                kicked.insert(
                    instance.clone(),
                    incidents
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                );
            }
        }
        Ok(IncidentLedger { kicked })
    }
}
